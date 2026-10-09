//! `nebula open <file>…` from inside an agent session: the files land in
//! every attached TUI as FILE TABS — one tab per file, previewed, editable —
//! so when the user asks to see a file the agent puts it in front of them
//! instead of pasting it into the reply. Only when asked: the guidance
//! below forbids opening anything unprompted. The CLI accepts text files,
//! local images and Mermaid diagrams, and refuses other binaries before
//! IPC. Like `nebula spawn`, nothing here touches the caller's process:
//! the model runs it, says what it opened, and carries on.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use nebula_core::{AgentId, ServerEvent};

use crate::registry::Daemon;

/// What nebula appends to Claude's system prompt so "show me the file"
/// becomes one `nebula open` call instead of the file's contents pasted
/// into the reply — and nothing else does: an agent wanting the user to
/// look at what it wrote names the path and waits to be asked. Claude and
/// pi, like the worktree and spawn guidance (all three take
/// `--append-system-prompt`): codex and cursor have no such flag.
pub const CLAUDE_OPEN_GUIDANCE: &str =
    "[nebula] Only when the user explicitly asks you to open or \
show a file, image, mockup or diagram in nebula (\"open it\", \"show me the file\", \"show the \
diagram\", \"open these in nebula\") — and never on \
your own initiative — save it to a local file when needed, then run this shell command, exactly \
once per set of files:\n\n  nebula open \
<file> [<file>…]\n\nwith paths relative to your working directory or absolute. A file you wrote or \
changed, a report, options for the user to pick from: none of these is a reason to open anything \
unasked — name the path in your reply and let the user ask. For diagrams, write a `.mmd` file or a \
```mermaid fenced block in Markdown; for mockups, write an image file (png, jpg, gif, webp, or svg). \
nebula shows text, images and Mermaid diagrams to the user at once, inside this app, as a tabbed \
modal — one tab per file, previewed, editable on Enter — so do not paste the files' contents into \
your reply as well: say in one line what you opened and carry on. If the command fails, report the \
error and name the paths instead.";

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

#[cfg(test)]
mod tests {
    use super::CLAUDE_OPEN_GUIDANCE as GUIDANCE;

    /// The guidance is all that stands between an agent and an unasked-for
    /// modal, so its two rules are pinned: opening is for explicit asks
    /// only, and for previewable local files only.
    #[test]
    fn the_open_guidance_is_explicit_ask_and_previewable_files_only() {
        assert!(GUIDANCE.starts_with("[nebula] Only when the user explicitly asks"));
        assert!(GUIDANCE.contains("never on your own initiative"));
        assert!(GUIDANCE.contains("name the path in your reply and let the user ask"));
        assert!(GUIDANCE.contains("image, mockup or diagram"));
        assert!(GUIDANCE.contains("```mermaid fenced block"));
        assert!(GUIDANCE.contains("nebula open <file> [<file>…]"));
        assert!(!GUIDANCE.contains("When you want the user to look at a file"));
    }
}
