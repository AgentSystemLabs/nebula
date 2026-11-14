//! GRAPHIFY: a code knowledge graph kept up to date for every checkout.
//!
//! When graphify is installed — or `uv` is, which fetches it on demand
//! (`uvx --from graphifyy graphify`) — the DAEMON runs `graphify update
//! <checkout>` — AST extraction only, no LLM, a few seconds on a large repo
//! and incremental after that — whenever a project is added, a worktree is
//! created, and an agent's turn finishes (the moment code most likely
//! changed). The result lands in `<checkout>/graphify-out/`, which is kept
//! out of git through the repository's own `info/exclude` (never a tracked
//! `.gitignore` edit), so it never shows in a diff.
//!
//! Claude sessions in a checkout that has a graph are told to query it
//! before searching broadly ([`claude_guidance`]), and the
//! read-only query commands are pre-approved alongside orbit's own.
//!
//! Off switches: `"graphify": false` in config.json for every project, or in
//! a checkout's `.orbit/config.json` for that one. A missing CLI is simply a no-op.
//!
//! Runs never overlap for one checkout: a refresh asked for while one is
//! running is folded into a single rerun when it ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Where graphify writes, relative to the checkout it was run on.
pub const OUT_DIR: &str = "graphify-out";

/// A run that takes longer than this is killed: a hung extractor must not
/// pin a CPU forever, and the next refresh simply tries again.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// Appended to Claude's system prompt in a checkout that has a graph.
/// `graphify` is how a shell types the tool here: `graphify`, or
/// `uvx --from graphifyy graphify` when uv fetches it on demand.
pub fn claude_guidance(graphify: &str) -> String {
    format!(
        "[orbit] This checkout has a code knowledge graph built by graphify in `graphify-out/` \
(orbit refreshes it automatically after every turn — never update it yourself). For questions \
about structure — where something lives, how parts connect, what a change would affect — query \
the graph before searching file by file:\n\n  {graphify} query \"<question>\"\n  {graphify} \
explain \"<symbol or file>\"\n  {graphify} path \"<A>\" \"<B>\"\n  {graphify} affected \"<symbol>\"\n\n\
then read the specific files it points to. `graphify-out/GRAPH_REPORT.md` summarises the \
architecture. Fall back to ordinary search when the graph has no answer."
    )
}

/// How a shell types graphify on this machine (`graphify` when nothing
/// resolves, so the guidance still reads sensibly).
pub fn shell_command() -> String {
    orbit_core::tools::graphify()
        .map(|g| g.shell)
        .unwrap_or_else(|| "graphify".into())
}

/// The guidance for `checkout`, when it has a graph.
pub fn guidance_for(checkout: &Path) -> Option<String> {
    has_graph(checkout).then(|| claude_guidance(&shell_command()))
}

/// Pre-approved permission rules for graphify's read-only queries, spelled
/// the way this machine runs it.
pub fn allow_rules() -> Vec<String> {
    let graphify = shell_command();
    ["query", "explain", "path", "affected", "god-nodes"]
        .iter()
        .map(|sub| format!("Bash({graphify} {sub}:*)"))
        .collect()
}

/// The graph's own file, whose presence means a checkout has a graph.
pub fn graph_json(checkout: &Path) -> PathBuf {
    checkout.join(OUT_DIR).join("graph.json")
}

/// True when the checkout has a graph — the condition for the guidance.
pub fn has_graph(checkout: &Path) -> bool {
    graph_json(checkout).is_file()
}

/// How to run graphify for `checkout`, when orbit should keep a graph
/// there: the setting is on, the checkout's `.orbit/config.json` doesn't opt out,
/// and graphify is installed or uv can fetch it.
pub fn wanted(checkout: &Path) -> Option<orbit_core::tools::Invocation> {
    if !crate::config::Config::load().graphify {
        return None;
    }
    if let Ok(Some(file)) = orbit_core::project_file::read(checkout) {
        if file.graphify == Some(false) {
            return None;
        }
    }
    orbit_core::tools::graphify()
}

#[derive(Default)]
struct Slot {
    /// Another refresh was asked for while this one ran.
    again: bool,
}

fn running() -> &'static Mutex<HashMap<PathBuf, Slot>> {
    static RUNNING: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Bring `checkout`'s graph up to date in the background. Returns at once;
/// does nothing when graphify is off, missing, or there is no runtime to
/// run on. A call while a run is in flight schedules one rerun.
pub fn refresh(checkout: &Path) {
    let Some(graphify) = wanted(checkout) else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let checkout = checkout.to_path_buf();
    {
        let mut map = running().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = map.get_mut(&checkout) {
            slot.again = true;
            return;
        }
        map.insert(checkout.clone(), Slot::default());
    }
    handle.spawn(async move {
        exclude_from_git(&checkout).await;
        loop {
            run_once(&graphify, &checkout).await;
            let mut map = running().lock().unwrap_or_else(|e| e.into_inner());
            match map.get_mut(&checkout) {
                Some(slot) if slot.again => slot.again = false,
                _ => {
                    map.remove(&checkout);
                    break;
                }
            }
        }
    });
}

async fn run_once(graphify: &orbit_core::tools::Invocation, checkout: &Path) {
    if !checkout.is_dir() {
        return;
    }
    // The first uvx run downloads graphify, so the timeout covers that too.
    let child = tokio::process::Command::new(&graphify.program)
        .args(&graphify.prefix)
        .arg("update")
        .arg(checkout)
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, "graphify: could not start");
            return;
        }
    };
    match tokio::time::timeout(RUN_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            tracing::debug!(checkout = %checkout.display(), "graphify: graph updated");
        }
        Ok(Ok(status)) => {
            tracing::warn!(checkout = %checkout.display(), %status, "graphify update failed");
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "graphify: wait failed"),
        Err(_) => {
            tracing::warn!(checkout = %checkout.display(), "graphify update timed out");
            let _ = child.kill().await;
        }
    }
}

/// Keep `graphify-out/` out of git for this checkout, unless something
/// already ignores it: append it to the repository's `info/exclude`, which
/// is local to the clone and shared by all its worktrees.
async fn exclude_from_git(checkout: &Path) {
    let git = |args: &[&str]| {
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
    };
    // The trailing slash asks about the directory even before it exists, so
    // a `graphify-out/` rule in .gitignore counts on a brand-new project.
    match git(&["check-ignore", "-q", "graphify-out/"]).await {
        Ok(out) if out.status.success() => return, // already ignored
        Ok(_) => {}
        Err(_) => return, // no git: nothing to keep out of
    }
    let Ok(out) = git(&["rev-parse", "--git-path", "info/exclude"]).await else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = {
        let p = PathBuf::from(&raw);
        if p.is_absolute() {
            p
        } else {
            checkout.join(p)
        }
    };
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# orbit: graphify's generated code graph\n/graphify-out/\n");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!(error = %e, path = %path.display(), "graphify: could not update info/exclude");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn has_graph_looks_for_graph_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!has_graph(tmp.path()));
        std::fs::create_dir_all(tmp.path().join(OUT_DIR)).unwrap();
        std::fs::write(graph_json(tmp.path()), "{}").unwrap();
        assert!(has_graph(tmp.path()));
    }

    #[tokio::test]
    async fn the_graph_is_excluded_once_and_never_shows_in_status() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        std::fs::create_dir_all(repo.join(OUT_DIR)).unwrap();
        std::fs::write(graph_json(repo), "{}").unwrap();

        exclude_from_git(repo).await;
        exclude_from_git(repo).await; // already ignored: nothing appended

        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("/graphify-out/").count(), 1, "{exclude}");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).is_empty(),
            "graphify-out must not show as untracked"
        );
    }

    #[tokio::test]
    async fn a_gitignored_graph_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        std::fs::write(repo.join(".gitignore"), "graphify-out/\n").unwrap();
        let before = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default();
        exclude_from_git(repo).await;
        let after = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default();
        assert_eq!(before, after);
    }
}
