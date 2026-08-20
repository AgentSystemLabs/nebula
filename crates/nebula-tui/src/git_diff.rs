//! Git status/diff readers for the diff modal.
//!
//! Synchronous `std::process` on purpose (the `pbcopy` precedent in
//! event_loop.rs): these run only on key events — opening the modal or
//! switching files — and per-file diffs are fast.

use crate::app::DiffView;
use std::path::Path;
use std::process::{Command, Output};

/// Keep pathological diffs from bloating the overlay state.
const MAX_DIFF_LINES: usize = 20_000;

/// One changed file from `git status --porcelain=v1 -z`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffFile {
    /// Path relative to the worktree root (for renames: the NEW path).
    pub path: String,
    /// Pre-rename path for R/C entries.
    pub orig_path: Option<String>,
    /// The two porcelain status columns, e.g. ['M',' '], [' ','M'], ['?','?'].
    pub xy: [char; 2],
}

impl DiffFile {
    pub fn is_untracked(&self) -> bool {
        self.xy == ['?', '?']
    }

    /// The raw two-character code for the list column ("M ", " M", "??", …).
    pub fn status_str(&self) -> String {
        self.xy.iter().collect()
    }
}

/// Line classification for coloring; styling itself lives in ui.rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Add,
    Remove,
    Hunk,
    Header,
    Context,
}

pub fn classify_diff_line(line: &str) -> DiffLineKind {
    const HEADERS: [&str; 11] = [
        "diff --git",
        "index ",
        "new file",
        "deleted file",
        "similarity ",
        "dissimilarity ",
        "rename ",
        "copy ",
        "old mode",
        "new mode",
        "Binary files",
    ];
    if line.starts_with("+++") || line.starts_with("---") {
        DiffLineKind::Header
    } else if line.starts_with('+') {
        DiffLineKind::Add
    } else if line.starts_with('-') {
        DiffLineKind::Remove
    } else if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if HEADERS.iter().any(|h| line.starts_with(h)) {
        DiffLineKind::Header
    } else {
        DiffLineKind::Context
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// Changed files (staged + unstaged + untracked) in status order.
/// `Err` is a user-facing flash message.
pub fn changed_files(root: &Path) -> Result<Vec<DiffFile>, String> {
    let output = run_git(root, &["status", "--porcelain=v1", "-z", "-uall"])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git status failed: {}", stderr.trim()));
    }
    Ok(parse_status_z(&output.stdout))
}

/// Parse NUL-separated porcelain v1: `XY path\0`, and for X ∈ {R, C} a second
/// NUL-terminated field holds the ORIGINAL path (`XY new\0old\0`).
pub fn parse_status_z(bytes: &[u8]) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut fields = bytes.split(|b| *b == 0);
    while let Some(entry) = fields.next() {
        let entry = String::from_utf8_lossy(entry);
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        let path = entry[3..].to_string();
        let orig_path = if matches!(x, 'R' | 'C') {
            fields
                .next()
                .map(|f| String::from_utf8_lossy(f).into_owned())
        } else {
            None
        };
        files.push(DiffFile {
            path,
            orig_path,
            xy: [x, y],
        });
    }
    files
}

/// Every file in the checkout (tracked + untracked, gitignore respected) in
/// git listing order, for the fuzzy file finder. `Err` is a user-facing
/// flash message.
pub fn list_files(root: &Path) -> Result<Vec<String>, String> {
    let output = run_git(
        root,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-files failed: {}", stderr.trim()));
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

/// Does this checkout have any commit? Unborn HEAD changes the diff command.
pub fn has_head(root: &Path) -> bool {
    head_oid(root).is_some()
}

/// HEAD's commit OID, `None` on an unborn HEAD (or outside a repo). The
/// review marks are scoped to this: any HEAD move invalidates them.
pub fn head_oid(root: &Path) -> Option<String> {
    let output = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Diff text for one file. Never fails: errors become the displayed text so
/// the modal survives a repo vanishing out from under it.
pub fn diff_for(root: &Path, file: &DiffFile, head_ok: bool) -> String {
    let output = if file.is_untracked() {
        // --no-index exits 1 when the files differ; only >= 2 is an error.
        run_git(
            root,
            &[
                "diff",
                "--no-index",
                "--no-color",
                "--",
                "/dev/null",
                &file.path,
            ],
        )
    } else {
        let mut args = vec!["diff"];
        if head_ok {
            args.push("HEAD");
        }
        args.extend(["--no-color", "--no-ext-diff", "--", &file.path]);
        if let Some(orig) = &file.orig_path {
            args.push(orig);
        }
        run_git(root, &args)
    };
    let output = match output {
        Ok(o) => o,
        Err(e) => return e,
    };
    let ok = if file.is_untracked() {
        matches!(output.status.code(), Some(0) | Some(1))
    } else {
        output.status.success()
    };
    if !ok {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return format!("git diff failed: {}", stderr.trim());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return "(no textual changes)".to_string();
    }
    let mut lines = text.lines();
    let mut out: String = lines
        .by_ref()
        .take(MAX_DIFF_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if lines.next().is_some() {
        out.push_str("\n… (truncated)");
    }
    out
}

/// Reload `view.diff` for the currently selected file and reset the scroll.
pub fn load_selected_diff(view: &mut DiffView) {
    let diff = match view.selected_file() {
        Some(file) => diff_for(&view.root, file, view.head_ok),
        None => String::new(),
    };
    view.diff_line_count = diff.lines().count();
    view.diff = diff;
    view.scroll = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_status_z_handles_all_statuses() {
        let bytes =
            b"M  staged.rs\0 M unstaged.rs\0?? new.txt\0D  gone.rs\0R  renamed.rs\0original.rs\0";
        let files = parse_status_z(bytes);
        assert_eq!(files.len(), 5);
        assert_eq!(files[0].path, "staged.rs");
        assert_eq!(files[0].xy, ['M', ' ']);
        assert_eq!(files[1].xy, [' ', 'M']);
        assert!(files[2].is_untracked());
        assert_eq!(files[2].status_str(), "??");
        assert_eq!(files[3].xy, ['D', ' ']);
        assert_eq!(files[4].path, "renamed.rs");
        assert_eq!(files[4].orig_path.as_deref(), Some("original.rs"));
        assert!(files[0].orig_path.is_none());
    }

    #[test]
    fn classify_diff_line_covers_headers_vs_adds() {
        assert_eq!(classify_diff_line("+added"), DiffLineKind::Add);
        assert_eq!(classify_diff_line("+++ b/file"), DiffLineKind::Header);
        assert_eq!(classify_diff_line("-removed"), DiffLineKind::Remove);
        assert_eq!(classify_diff_line("--- a/file"), DiffLineKind::Header);
        assert_eq!(classify_diff_line("@@ -1,3 +1,4 @@"), DiffLineKind::Hunk);
        assert_eq!(
            classify_diff_line("diff --git a/x b/x"),
            DiffLineKind::Header
        );
        assert_eq!(
            classify_diff_line("index 123..456 100644"),
            DiffLineKind::Header
        );
        assert_eq!(
            classify_diff_line("Binary files a/x and b/x differ"),
            DiffLineKind::Header
        );
        assert_eq!(classify_diff_line(" context"), DiffLineKind::Context);
    }

    fn git(repo: &PathBuf, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn make_repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("tracked.txt"), "old line\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        repo
    }

    #[test]
    fn changed_files_and_diff_for_from_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("tracked.txt"), "new line\n").unwrap();
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();

        let files = changed_files(&repo).unwrap();
        assert_eq!(files.len(), 2);
        let tracked = files.iter().find(|f| f.path == "tracked.txt").unwrap();
        let fresh = files.iter().find(|f| f.path == "fresh.txt").unwrap();
        assert!(fresh.is_untracked());

        assert!(has_head(&repo));
        let diff = diff_for(&repo, tracked, true);
        assert!(diff.contains("-old line"), "{diff}");
        assert!(diff.contains("+new line"), "{diff}");
        // Untracked goes through the --no-index exit-1 path.
        let diff = diff_for(&repo, fresh, true);
        assert!(diff.contains("+hello"), "{diff}");
    }

    #[test]
    fn list_files_includes_untracked_and_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "nope\n").unwrap();

        let files = list_files(&repo).unwrap();
        assert!(files.contains(&"tracked.txt".to_string()), "{files:?}");
        assert!(files.contains(&"fresh.txt".to_string()), "{files:?}");
        assert!(!files.contains(&"ignored.txt".to_string()), "{files:?}");
    }

    #[test]
    fn list_files_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = list_files(dir.path()).unwrap_err();
        assert!(err.contains("git ls-files failed"), "{err}");
    }

    #[test]
    fn head_oid_moves_with_commits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let first = head_oid(&repo).expect("committed repo has a HEAD");
        std::fs::write(repo.join("tracked.txt"), "new line\n").unwrap();
        git(&repo, &["commit", "-am", "second"]);
        let second = head_oid(&repo).expect("still has a HEAD");
        assert_ne!(first, second, "a commit moves the OID");
        assert!(head_oid(dir.path()).is_none(), "no repo, no OID");
    }

    #[test]
    fn unborn_head_falls_back_to_plain_diff() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("only.txt"), "content\n").unwrap();

        assert!(!has_head(&repo));
        let files = changed_files(&repo).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].is_untracked());
        let diff = diff_for(&repo, &files[0], false);
        assert!(diff.contains("+content"), "{diff}");
    }

    #[test]
    fn changed_files_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = changed_files(dir.path()).unwrap_err();
        assert!(err.contains("git status failed"), "{err}");
    }
}
