//! The DELETE CHECK: what taking a worktree off disk would interrupt or
//! lose, gathered for the confirm a client shows before it does.
//!
//! nebula's own rows are not enough to answer that. An agent whose session
//! lives in one checkout often works in another through absolute paths —
//! a background subagent's tool calls carry its parent's working directory
//! — so a worktree whose band reads "nothing running" can have a build
//! running in it and an hour of edits nobody committed. Three questions,
//! each cheap and each best effort, and each saying so when it could not
//! look: which processes have their working directory inside the checkout,
//! what `git status` reports there and how recently it was written, and
//! which commits HEAD has that its base lacks.

use crate::git;
use anyhow::{Context, Result};
use nebula_core::{WorktreeCheck, WorktreeProcess};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Run the DELETE CHECK on `worktree`, a checkout of `repo`. `spare` is
/// every pid the worktree's own sessions lead or run — the confirm counts
/// those already and they go down with it — and `configured_base` the
/// WORKTREE BASE BRANCH setting.
pub async fn check(
    repo: &Path,
    worktree: &Path,
    spare: HashSet<u32>,
    configured_base: Option<String>,
) -> WorktreeCheck {
    if !worktree.exists() {
        // Already gone: a delete only drops the row, and loses nothing.
        return WorktreeCheck::default();
    }
    // The sweep first, then git: run side by side, the daemon's own
    // `git -C <checkout>` (and the submodule gits under it) would be in
    // the sweep, working in the checkout.
    let dir = worktree.to_path_buf();
    let sweep = tokio::task::spawn_blocking(move || processes_in(&dir, &spare)).await;
    let mut check = git_half(repo, worktree, configured_base).await;
    match sweep {
        Ok(Ok(processes)) => check.processes = processes,
        Ok(Err(e)) => check.process_error = Some(format!("{e:#}")),
        Err(e) => check.process_error = Some(format!("process sweep failed: {e}")),
    }
    check
}

/// What git has to say about the checkout: its changed paths and how
/// recently they were written, and its commits the base lacks — or, on a
/// detached HEAD, the commits nothing else holds.
async fn git_half(repo: &Path, worktree: &Path, configured_base: Option<String>) -> WorktreeCheck {
    let mut check = WorktreeCheck::default();
    // The first thing git could not answer is the one the confirm names.
    let note = |e: anyhow::Error, check: &mut WorktreeCheck| {
        check.git_error.get_or_insert_with(|| format!("{e:#}"));
    };
    match git::changed_paths(worktree).await {
        Ok(paths) => {
            check.changes = u32::try_from(paths.len()).unwrap_or(u32::MAX);
            check.newest_change_ms = newest_write_ms(worktree, &paths);
        }
        Err(e) => note(e, &mut check),
    }
    if git::head_detached(worktree).await {
        check.detached = true;
        match git::unreferenced_commits(worktree).await {
            Ok(n) => check.ahead = n,
            Err(e) => note(e, &mut check),
        }
    } else if let Some(base) = git::comparison_base(repo, configured_base.as_deref()).await {
        match git::commits_ahead(worktree, &base).await {
            Ok(n) => check.ahead = n,
            Err(e) => note(e, &mut check),
        }
        check.base = Some(base);
    } else {
        // Nothing to compare with is not "nothing its base lacks".
        note(
            anyhow::anyhow!("found no base branch to compare its commits with"),
            &mut check,
        );
    }
    check
}

/// Epoch ms of the newest modification among `paths` (relative to
/// `root`); a path that is gone — a deletion git reports — has none.
fn newest_write_ms(root: &Path, paths: &[PathBuf]) -> Option<i64> {
    paths
        .iter()
        .filter_map(|p| {
            std::fs::symlink_metadata(root.join(p))
                .ok()?
                .modified()
                .ok()
        })
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .max()
}

/// Every process whose working directory is `dir` or below it, by pid,
/// other than `spare` and this daemon.
pub fn processes_in(dir: &Path, spare: &HashSet<u32>) -> Result<Vec<WorktreeProcess>> {
    Ok(processes_under(&process_cwds()?, dir, spare))
}

/// Pure core of [`processes_in`] over a sweep of (pid, name, cwd).
fn processes_under(
    sweep: &[(u32, String, PathBuf)],
    dir: &Path,
    spare: &HashSet<u32>,
) -> Vec<WorktreeProcess> {
    // The OS reports resolved paths (`/private/tmp/...` on macOS); the row
    // may hold the unresolved one. Either spelling counts.
    let mut roots = vec![dir.to_path_buf()];
    if let Ok(real) = std::fs::canonicalize(dir) {
        if real != dir {
            roots.push(real);
        }
    }
    let me = std::process::id();
    let mut found: Vec<WorktreeProcess> = sweep
        .iter()
        .filter(|(pid, _, cwd)| {
            *pid != me && !spare.contains(pid) && roots.iter().any(|r| cwd.starts_with(r))
        })
        .map(|(pid, name, _)| WorktreeProcess {
            pid: *pid,
            name: name.clone(),
        })
        .collect();
    found.sort_by_key(|p| p.pid);
    found
}

/// Every process this user can see, with its working directory: read off
/// `/proc` on Linux, where it costs a readlink per process.
#[cfg(target_os = "linux")]
fn process_cwds() -> Result<Vec<(u32, String, PathBuf)>> {
    let mut sweep = Vec::new();
    for entry in std::fs::read_dir("/proc").context("read /proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        // Another user's process, or one that exited mid-sweep.
        let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) else {
            continue;
        };
        let name = std::fs::read_to_string(entry.path().join("comm"))
            .map(|n| n.trim().to_string())
            .unwrap_or_default();
        sweep.push((pid, name, cwd));
    }
    Ok(sweep)
}

/// Every process this user can see, with its working directory: one
/// `lsof` asked for the `cwd` of everything, a few hundred ms on a busy
/// Mac — this runs once per confirm, never on a beat.
#[cfg(not(target_os = "linux"))]
fn process_cwds() -> Result<Vec<(u32, String, PathBuf)>> {
    let out = std::process::Command::new("lsof")
        .args(["-w", "-a", "-d", "cwd", "-F", "pcn"])
        .output()
        .context("run lsof")?;
    // lsof exits 1 when some process could not be read (another user's);
    // what it did print is still good. Nothing at all is a failure.
    if out.stdout.is_empty() && !out.status.success() {
        anyhow::bail!(
            "lsof failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_lsof(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `lsof -F pcn` field output: a `p<pid>` line opens each process,
/// `c<name>` names it, and `n<path>` is the file asked about — here its
/// working directory. Other fields (`f`, always printed) are skipped.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lsof(out: &str) -> Vec<(u32, String, PathBuf)> {
    let mut sweep = Vec::new();
    let mut pid = None;
    let mut name = String::new();
    for line in out.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];
        match tag {
            'p' => {
                pid = value.parse().ok();
                name.clear();
            }
            'c' => name = value.to_string(),
            'n' => {
                if let Some(pid) = pid {
                    sweep.push((pid, name.clone(), PathBuf::from(value)));
                }
            }
            _ => {}
        }
    }
    sweep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_fields_parse_into_pid_name_and_cwd() {
        let out = "p101\ncnvim\nfcwd\nn/work/feat\np102\nczsh\nfcwd\nn/work/feat 2/src\np103\n";
        assert_eq!(
            parse_lsof(out),
            vec![
                (101, "nvim".to_string(), PathBuf::from("/work/feat")),
                (102, "zsh".to_string(), PathBuf::from("/work/feat 2/src")),
            ]
        );
    }

    #[test]
    fn processes_under_match_whole_components_and_skip_spared_pids() {
        let sweep = vec![
            (10, "cargo".into(), PathBuf::from("/w/feat/crates/x")),
            (11, "zsh".into(), PathBuf::from("/w/feat")),
            (12, "zsh".into(), PathBuf::from("/w/feature")),
            (13, "claude".into(), PathBuf::from("/w/main")),
            (14, "vim".into(), PathBuf::from("/w/feat")),
            (
                std::process::id(),
                "nebula".into(),
                PathBuf::from("/w/feat"),
            ),
        ];
        let spare: HashSet<u32> = [14].into();
        let got: Vec<u32> = processes_under(&sweep, Path::new("/w/feat"), &spare)
            .into_iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(got, vec![10, 11], "not /w/feature, not spared, not us");
    }

    /// The sweep on this machine finds a real process sitting in a
    /// directory — a `sleep` started there — and spares it on request.
    #[test]
    fn the_live_sweep_finds_a_process_working_in_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("feat");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(dir.join("src"))
            .spawn()
            .unwrap();
        let pid = child.id();
        let found = processes_in(&dir, &HashSet::new());
        let spared = processes_in(&dir, &[pid].into());
        let _ = child.kill();
        let _ = child.wait();
        let found = found.unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.pid == pid && p.name.contains("sleep")),
            "{found:?}"
        );
        assert!(!spared.unwrap().iter().any(|p| p.pid == pid));
    }

    /// A checkout already gone from disk has nothing left to lose.
    #[tokio::test]
    async fn a_missing_checkout_checks_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        let check = check(tmp.path(), &gone, HashSet::new(), None).await;
        assert!(!check.has_findings(), "{check:?}");
    }
}
