//! The PROJECT FILE: `.nebula.json`, committed at the root of a repository,
//! telling nebula how the project is run and how to open what it serves.
//!
//! ```json
//! {
//!   "run": "npm run dev",
//!   "open": "open http://localhost:3000"
//! }
//! ```
//!
//! Both values are shell command lines, and both run in the selected
//! worktree's checkout. `run` is the RUN COMMAND: a menu's **Run** starts
//! it in a RUN TERMINAL the DAEMON holds, and **Stop run** stops it. `open`
//! is the OPEN COMMAND: `Shift+Enter` on a worktree fires it once, from the
//! TUI.
//!
//! The file is read fresh at every press — from the worktree's own checkout
//! first, so a branch can carry commands of its own, and otherwise from the
//! project's main checkout, so a file not yet merged into an older branch
//! still applies. The first file found is the whole answer: a worktree's
//! file that leaves `open` out does not borrow the main checkout's.
//!
//! Unlike WORKTREE HOOKS, which nebula runs on its own after a create or a
//! delete and so never takes from a checkout, nothing here runs unless the
//! user presses the key on that worktree — the same trust as typing the
//! command into a shell there.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The file's name at the root of a checkout.
pub const FILE_NAME: &str = ".nebula.json";

/// What `.nebula.json` says. Unknown keys are ignored, so a file written
/// for a newer nebula still works in this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ProjectFile {
    /// The RUN COMMAND, started and stopped with `r`.
    #[serde(default)]
    pub run: Option<String>,
    /// The OPEN COMMAND, fired with `Shift+Enter`.
    #[serde(default)]
    pub open: Option<String>,
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

/// Parse one checkout's file. Ok(None) when the checkout has none.
pub fn read(dir: &Path) -> Result<Option<ProjectFile>, String> {
    let path = dir.join(FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The file that applies to a worktree, and the checkout it came from: the
/// worktree's own, else the main checkout's.
pub fn find(worktree: &Path, main: &Path) -> Result<Option<(PathBuf, ProjectFile)>, String> {
    let mut dirs = vec![worktree];
    if main != worktree {
        dirs.push(main);
    }
    for dir in dirs {
        if let Some(file) = read(dir)? {
            return Ok(Some((dir.to_path_buf(), file)));
        }
    }
    Ok(None)
}

/// The command line `which` names for a worktree: Ok(None) when there is
/// no file, or the file leaves the key out or blank, so a caller with a
/// fallback of its own can take it; Err only for a file that is there and
/// can't be read. The DAEMON's run path looks here after the project's
/// own `run_command` setting.
pub fn lookup(
    worktree: &Path,
    main: &Path,
    which: ProjectCommand,
) -> Result<Option<String>, String> {
    Ok(find(worktree, main)?.and_then(|(_, file)| file.command(which).map(str::to_string)))
}

/// The command line `which` names for a worktree, or the one-line reason
/// there is none — short enough for the footer flash, and saying what to
/// add.
pub fn command(worktree: &Path, main: &Path, which: ProjectCommand) -> Result<String, String> {
    let key = which.key();
    let example = which.example();
    match find(worktree, main)? {
        Some((_, file)) => file.command(which).map(str::to_string).ok_or_else(|| {
            format!("{FILE_NAME} has no \"{key}\" command — add \"{key}\": \"{example}\"")
        }),
        None => Err(format!(
            "no {FILE_NAME} in this worktree — add one with {{\"{key}\": \"{example}\"}}"
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
            let dir = std::env::temp_dir()
                .join(format!("nebula-project-file-{}", ulid::Ulid::generate()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn with_file(self, json: &str) -> Self {
            std::fs::write(self.0.join(FILE_NAME), json).unwrap();
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
        let run = command(&dir.0, &dir.0, ProjectCommand::Run).unwrap();
        let open = command(&dir.0, &dir.0, ProjectCommand::Open).unwrap();
        assert_eq!(run, "npm run dev");
        assert_eq!(open, "open http://localhost:3000", "trimmed");
    }

    #[test]
    fn the_worktree_file_wins_over_the_main_checkout() {
        let worktree = TempDir::new().with_file(r#"{"run": "bun dev"}"#);
        let main = TempDir::new().with_file(r#"{"run": "npm run dev", "open": "open x"}"#);
        assert_eq!(
            command(&worktree.0, &main.0, ProjectCommand::Run).unwrap(),
            "bun dev"
        );
        // The first file found is the whole answer — no borrowing a key.
        let err = command(&worktree.0, &main.0, ProjectCommand::Open).unwrap_err();
        assert!(err.contains("has no \"open\" command"), "{err}");
    }

    #[test]
    fn a_worktree_without_a_file_falls_back_to_the_main_checkout() {
        let worktree = TempDir::new();
        let main = TempDir::new().with_file(r#"{"open": "open http://localhost:5173"}"#);
        assert_eq!(
            command(&worktree.0, &main.0, ProjectCommand::Open).unwrap(),
            "open http://localhost:5173"
        );
    }

    /// `lookup` is `command` without the message: nothing to run is
    /// Ok(None), for the caller that has somewhere else to look, and only
    /// a file that is there but unreadable is an error.
    #[test]
    fn lookup_reads_none_for_a_missing_file_or_key_and_keeps_the_parse_error() {
        let empty = TempDir::new();
        assert_eq!(lookup(&empty.0, &empty.0, ProjectCommand::Run), Ok(None));
        let blank = TempDir::new().with_file(r#"{"run": "   ", "open": "open x"}"#);
        assert_eq!(lookup(&blank.0, &blank.0, ProjectCommand::Run), Ok(None));
        assert_eq!(
            lookup(&blank.0, &blank.0, ProjectCommand::Open),
            Ok(Some("open x".into()))
        );
        let broken = TempDir::new().with_file("{");
        let err = lookup(&broken.0, &broken.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains(FILE_NAME), "{err}");
    }

    #[test]
    fn missing_file_blank_command_and_bad_json_say_what_to_fix() {
        let empty = TempDir::new();
        let err = command(&empty.0, &empty.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains("no .nebula.json"), "{err}");
        assert!(err.contains(r#"{"run": "npm run dev"}"#), "{err}");

        let blank = TempDir::new().with_file(r#"{"run": "   "}"#);
        let err = command(&blank.0, &blank.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains("has no \"run\" command"), "{err}");

        let broken = TempDir::new().with_file(r#"{"run": ["npm", "dev"]}"#);
        let err = command(&broken.0, &broken.0, ProjectCommand::Run).unwrap_err();
        assert!(err.contains(FILE_NAME), "names the file: {err}");
        assert!(err.contains("invalid type"), "{err}");
    }
}
