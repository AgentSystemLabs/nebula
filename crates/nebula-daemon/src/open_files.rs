//! `nebula open <file>…` from inside an agent session: the files land in
//! every attached TUI as FILE TABS — one tab per file, previewed, editable —
//! so an agent can put a file in front of the user instead of pasting it
//! into the reply. Like `nebula spawn`, nothing here touches the caller's
//! process: the model runs it, says what it opened, and carries on.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use nebula_core::{AgentId, ServerEvent};

use crate::registry::Daemon;

/// What nebula appends to Claude's system prompt so "show me the file" —
/// or an agent wanting the user to look at what it wrote — becomes one
/// `nebula open` call instead of the file's contents pasted into the
/// reply. Claude and pi, like the worktree and spawn guidance (all three
/// take `--append-system-prompt`): codex and cursor have no such flag.
pub const CLAUDE_OPEN_GUIDANCE: &str = "[nebula] When you want the user to look at a file — one \
you wrote or changed, examples or options for them to pick from, a report — or when they ask you \
to open or show a file in nebula (\"open it\", \"show me the file\", \"open these in nebula\"), run \
this shell command, exactly once per set of files:\n\n  nebula open <file> [<file>…]\n\nwith paths \
relative to your working directory or absolute. nebula shows them to the user at once, inside this \
app, as a tabbed modal — one tab per file, previewed, editable on Enter — so do not paste the \
files' contents into your reply as well: say in one line what you opened and carry on. If the \
command fails, report the error and name the paths instead.";

impl Daemon {
    /// `nebula open`, run by the agent inside its own session: the caller
    /// has to be a known row (its worktree is the editor's cwd on the other
    /// side), and then the files go to every subscriber at once. No file is
    /// read here — the CLI already resolved and checked them, and the TUI
    /// reads what it shows.
    pub fn open_files(&self, id: &AgentId, paths: Vec<PathBuf>) -> Result<()> {
        if paths.is_empty() {
            bail!("no files to open");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("the session's worktree is gone")?;
        self.broadcast(ServerEvent::FilesOpened {
            agent: id.clone(),
            root: worktree.path,
            paths,
        });
        Ok(())
    }
}
