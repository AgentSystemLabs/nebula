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
//! which commits HEAD has that its base lacks. And one thing a person or
//! tool may have said outright: that the checkout is locked, and why.

use crate::git;
use anyhow::{Context, Result};
use nebula_core::{WorktreeCheck, WorktreeLanding, WorktreeProcess};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Ignored files a confirm names before it says "and N more".
const IGNORED_NAMED: usize = 3;

/// Run the DELETE CHECK on `worktree`, a checkout of `repo`. `spare` is
/// every pid the worktree's own sessions lead or run — the confirm counts
/// those already and they go down with it — and `configured_base` the
/// WORKTREE BASE BRANCH setting. `band` is an idle band's check, asked
/// every rotation: it skips the gitignored files and the submodules'
/// remote tags, a confirm's questions only, and keeps the rest — the
/// processes at work in the checkout
/// included, which is how a band says it is in use.
pub async fn check(
    repo: &Path,
    worktree: &Path,
    spare: HashSet<u32>,
    configured_base: Option<String>,
    band: bool,
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
    let mut check = git_half(repo, worktree, configured_base, !band).await;
    match sweep {
        Ok(Ok(processes)) => check.processes = processes,
        Ok(Err(e)) => check.process_error = Some(format!("{e:#}")),
        Err(e) => check.process_error = Some(format!("process sweep failed: {e}")),
    }
    // A git at work in the checkout (a `git submodule update` still
    // cloning, a rebase) puts its state in flux, which a band says
    // instead of reading it.
    let op = match live_git_op(&check.processes).await {
        Some(op) => Some((op, true)),
        None => paused_git_op(worktree).map(|op| (op, false)),
    };
    note_op(&mut check, op);
    check
}

/// Record a git operation found under way in the checkout.
fn note_op(check: &mut WorktreeCheck, op: Option<(String, bool)>) {
    if let (Some((op, live)), Some(landing)) = (op, check.landing.as_mut()) {
        landing.git_op = Some(op);
        landing.git_op_live = live;
    }
}

/// A git operation stopped part-way in the checkout, as its git dir's
/// state says: a rebase (`rebase-merge/`, `rebase-apply/`), an `am`, a
/// merge, a cherry-pick or a revert waiting on the user.
fn paused_git_op(worktree: &Path) -> Option<String> {
    let git_dir = git::store_of(worktree)?;
    let op = if git_dir.join("rebase-merge").is_dir() {
        "rebase"
    } else if git_dir.join("rebase-apply").join("applying").exists() {
        "am"
    } else if git_dir.join("rebase-apply").is_dir() {
        "rebase"
    } else if git_dir.join("MERGE_HEAD").exists() {
        "merge"
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        "cherry-pick"
    } else if git_dir.join("REVERT_HEAD").exists() {
        "revert"
    } else {
        return None;
    };
    Some(op.to_string())
}

/// Git commands that change a checkout's state while they run — what a
/// band waits out. Reads (`status`, `rev-list`, nebula's own) are not.
const WRITING_GIT: &[&str] = &[
    "submodule",
    "clone",
    "fetch",
    "pull",
    "rebase",
    "merge",
    "am",
    "cherry-pick",
    "revert",
    "checkout",
    "switch",
];

/// The git operation one of `processes` (working in the checkout) is
/// running, from its command line: the oldest such process's, which is
/// the one the user started (`git submodule update` over the clones it
/// runs).
async fn live_git_op(processes: &[WorktreeProcess]) -> Option<String> {
    let pids: Vec<String> = processes
        .iter()
        .filter(|p| p.name.starts_with("git"))
        .map(|p| p.pid.to_string())
        .collect();
    if pids.is_empty() {
        return None;
    }
    let out = tokio::process::Command::new("ps")
        .args(["-o", "pid=,args=", "-p", &pids.join(",")])
        .output()
        .await
        .ok()?;
    let mut ops: Vec<(u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, args) = line.trim().split_once(' ')?;
            Some((pid.parse().ok()?, git_op_of(args)?))
        })
        .collect();
    ops.sort();
    ops.into_iter().next().map(|(_, op)| op)
}

/// The operation a git command line runs (`git -c x=y submodule update
/// --init` is `submodule update`), when it is one of [`WRITING_GIT`].
fn git_op_of(args: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    let program = words.next()?;
    if !program.rsplit('/').next()?.starts_with("git") {
        return None;
    }
    let mut words = words.peekable();
    // Skip the global options, and the value of those that take one.
    while let Some(word) = words.peek() {
        match *word {
            "-c" | "-C" => {
                words.next();
                words.next();
            }
            w if w.starts_with('-') => {
                words.next();
            }
            _ => break,
        }
    }
    let command = words.next()?;
    let command = command.strip_suffix("--helper").unwrap_or(command);
    if !WRITING_GIT.contains(&command) {
        return None;
    }
    if command == "submodule" {
        if let Some(sub) = words.find(|w| !w.starts_with('-')) {
            return Some(format!("submodule {sub}"));
        }
    }
    Some(command.to_string())
}

/// What git has to say about the checkout: its changed paths and how
/// recently they were written, its commits the base lacks by content — or,
/// on a detached HEAD, the commits nothing else holds — and its
/// submodules' commits that live in this checkout's store alone.
/// `confirm` also lists the gitignored files a delete takes and asks the
/// submodules' remotes which of their tags they have: a confirm's
/// questions, not a band's, which would pay for them every rotation.
async fn git_half(
    repo: &Path,
    worktree: &Path,
    configured_base: Option<String>,
    confirm: bool,
) -> WorktreeCheck {
    let mut check = WorktreeCheck::default();
    let mut landing = WorktreeLanding::default();
    // The four questions are independent; asked side by side, the check
    // takes as long as the slowest (the submodules, in a repo with many).
    let (status, commits, submodules, keepsakes) = tokio::join!(
        git::changed_paths(worktree),
        commits_half(repo, worktree, configured_base),
        git::submodule_unpushed(worktree, repo, confirm),
        async {
            match confirm {
                true => Some(git::ignored_keepsakes(worktree).await),
                false => None,
            }
        },
    );
    // The first thing git could not answer is the one the confirm names.
    let note = |e: anyhow::Error, check: &mut WorktreeCheck| {
        check.git_error.get_or_insert_with(|| format!("{e:#}"));
    };
    match status {
        Ok(paths) => {
            check.changes = u32::try_from(paths.len()).unwrap_or(u32::MAX);
            let untracked = paths.iter().filter(|p| p.untracked).count();
            landing.untracked = u32::try_from(untracked).unwrap_or(u32::MAX);
            let paths: Vec<PathBuf> = paths.into_iter().map(|p| p.path).collect();
            check.newest_change_ms = newest_write_ms(worktree, &paths);
        }
        Err(e) => note(e, &mut check),
    }
    check.detached = commits.detached;
    check.ahead = commits.ahead;
    check.base = commits.base;
    landing.branch_landed = commits.branch_landed;
    landing.branch = commits.tip;
    if let Some(e) = commits.error {
        note(e, &mut check);
    }
    match submodules {
        Ok(n) => landing.submodule_unpushed = n,
        Err(e) => {
            // Which submodule, when it is the failure the check names.
            if check.git_error.is_none() {
                landing.failed_submodule = e
                    .downcast_ref::<git::SubmoduleError>()
                    .map(|failed| failed.path.clone());
            }
            note(e.context("submodules"), &mut check);
        }
    }
    match keepsakes {
        Some(Ok(files)) => {
            landing.ignored_total = u32::try_from(files.len()).unwrap_or(u32::MAX);
            landing.ignored_files = files.into_iter().take(IGNORED_NAMED).collect();
        }
        Some(Err(e)) => note(e.context("ignored files"), &mut check),
        None => {}
    }
    match git::worktree_lock(repo, worktree).await {
        Ok(lock) => check.locked = lock,
        Err(e) => note(e, &mut check),
    }
    check.landing = Some(landing);
    check
}

/// What [`commits_half`] found about the checkout's commits.
#[derive(Default)]
struct Commits {
    detached: bool,
    ahead: u32,
    base: Option<String>,
    tip: Option<nebula_core::BranchTip>,
    branch_landed: bool,
    /// The first thing git could not answer.
    error: Option<anyhow::Error>,
}

/// The checkout's commits its base lacks by content — or, on a detached
/// HEAD, the commits nothing else holds — and where its branch stands.
async fn commits_half(repo: &Path, worktree: &Path, configured_base: Option<String>) -> Commits {
    let mut out = Commits::default();
    if git::head_detached(worktree).await {
        out.detached = true;
        match git::unreferenced_commits(worktree).await {
            Ok(n) => out.ahead = n,
            Err(e) => out.error = Some(e),
        }
    } else if let Some(base) = git::comparison_base(repo, configured_base.as_deref()).await {
        // The branch and tip judged here are the ones a confirm hands back
        // to the delete: counted at that tip, so the verdict and the
        // commit the delete compares against are the same.
        let tip = git::branch_tip(worktree).await;
        let head = tip.as_ref().map_or("HEAD", |t| t.oid.as_str());
        match git::unlanded_commits(worktree, &base, head).await {
            Ok(n) => {
                out.ahead = n;
                out.branch_landed = n == 0
                    && match &tip {
                        Some(t) => !git::protected_branch(repo, &t.name, Some(&base)).await,
                        None => false,
                    };
            }
            Err(e) => out.error = Some(e),
        }
        out.tip = tip;
        out.base = Some(base);
    } else {
        // Nothing to compare with is not "nothing its base lacks".
        out.error = Some(anyhow::anyhow!(
            "found no base branch to compare its commits with"
        ));
    }
    out
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
/// `lsof` asked for the `cwd` of everything, under a tenth of a second
/// to a few hundred ms on a busy Mac. It runs once per confirm, and once
/// per idle band's check (at most one each git tick).
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
    #[test]
    fn a_git_command_line_names_its_operation() {
        assert_eq!(
            git_op_of("git -c protocol.file.allow=always submodule update --init").as_deref(),
            Some("submodule update")
        );
        assert_eq!(
            git_op_of("/usr/libexec/git-core/git submodule--helper update --init").as_deref(),
            Some("submodule update")
        );
        assert_eq!(
            git_op_of("git -C /w rebase -i main").as_deref(),
            Some("rebase")
        );
        assert_eq!(git_op_of("git status --porcelain"), None);
        assert_eq!(git_op_of("cargo build"), None);
    }

    /// A rebase stopped part-way is a state, not a failure: the check says
    /// which, and that it is not running on its own.
    #[tokio::test]
    async fn a_stopped_rebase_is_named() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            out.status.success()
        };
        assert!(run(&["init", "-q", "-b", "main"]));
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        assert!(run(&["add", "a.txt"]));
        assert!(run(&["commit", "-q", "-m", "base"]));
        assert!(run(&["switch", "-q", "-c", "feat"]));
        std::fs::write(repo.join("a.txt"), "feat\n").unwrap();
        assert!(run(&["commit", "-q", "-am", "feat"]));
        assert!(run(&["switch", "-q", "main"]));
        std::fs::write(repo.join("a.txt"), "main\n").unwrap();
        assert!(run(&["commit", "-q", "-am", "main"]));
        assert!(run(&["switch", "-q", "feat"]));
        assert!(!run(&["rebase", "-q", "main"]), "conflicts and stops");
        let check = check(&repo, &repo, HashSet::new(), None, true).await;
        let landing = check.landing.expect("a landing");
        assert_eq!(landing.git_op.as_deref(), Some("rebase"));
        assert!(!landing.git_op_live);
    }

    /// A `git submodule update` at work in the checkout is named, live,
    /// by a band's check and a confirm's — slowed here by a hook that
    /// sleeps on every ref update, so the check lands while it runs.
    #[tokio::test]
    async fn a_running_submodule_update_is_named() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        let repo = root.join("repo");
        let up = root.join("up");
        for dir in [&repo, &up] {
            std::fs::create_dir(dir).unwrap();
            run(dir, &["init", "-q", "-b", "main"]);
            run(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
        }
        let url = up.to_string_lossy().into_owned();
        run(
            &repo,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                "sub",
            ],
        );
        run(&repo, &["commit", "-q", "-m", "sub"]);
        let wt = root.join("wt");
        run(
            &repo,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "feat"],
        );
        let hooks = root.join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("reference-transaction");
        std::fs::write(&hook, "#!/bin/sh\ncat >/dev/null\nsleep 2\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::process::CommandExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut update = std::process::Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args([
                "-c",
                &format!("core.hooksPath={}", hooks.display()),
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ])
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut seen = None;
        for _ in 0..40 {
            let band = check(&repo, &wt, HashSet::new(), None, true).await;
            if let Some(op) = band.landing.and_then(|l| l.git_op_live.then_some(l.git_op)) {
                seen = op;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let confirm = check(&repo, &wt, HashSet::new(), None, false).await;
        // The update and the clone under it, before the repo goes.
        let group = nix::unistd::Pid::from_raw(update.id() as i32);
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        let _ = update.wait();
        assert_eq!(seen.as_deref(), Some("submodule update"));
        let landing = confirm.landing.expect("a landing");
        assert!(landing.git_op.is_some(), "{landing:?}");
    }

    /// A submodule git cannot read fails the check, naming the submodule.
    #[tokio::test]
    async fn a_submodule_git_cannot_read_is_named() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let broken = Path::new(run(&["rev-parse", "--absolute-git-dir"]).trim())
            .join("modules")
            .join("half");
        std::fs::create_dir_all(broken.join("objects")).unwrap();
        std::fs::write(broken.join("HEAD"), "not a ref\n").unwrap();
        let check = check(&repo, &repo, HashSet::new(), None, true).await;
        assert!(check.git_error.is_some(), "{check:?}");
        let landing = check.landing.expect("a landing");
        assert_eq!(landing.failed_submodule.as_deref(), Some("half"));
    }

    #[tokio::test]
    async fn a_missing_checkout_checks_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        let check = check(tmp.path(), &gone, HashSet::new(), None, false).await;
        assert!(!check.has_findings(), "{check:?}");
    }
}
