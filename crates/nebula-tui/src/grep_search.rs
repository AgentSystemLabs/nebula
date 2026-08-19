//! `git grep` runner for the find-in-files overlay.
//!
//! Synchronous `std::process` on purpose (the git_diff.rs precedent): a
//! search runs only on key events, and `git grep` over a checkout is fast.

use std::path::Path;
use std::process::Command;

/// Queries shorter than this don't search — one character would light up
//  half the repo.
pub const MIN_QUERY_LEN: usize = 2;
/// Hits beyond this are dropped (`truncated` tells the UI to say so).
pub const MAX_RESULTS: usize = 200;
/// Matched lines are clipped to this many chars to keep overlay state small.
const MAX_TEXT_LEN: usize = 250;

/// One `git grep` match.
#[derive(Debug, Clone, PartialEq)]
pub struct GrepHit {
    /// Path relative to the search root.
    pub path: String,
    /// 1-based line number.
    pub line: u64,
    /// The matched line, trimmed and clipped.
    pub text: String,
}

/// Fixed-string smart-case search over the checkout (tracked + untracked,
/// gitignore respected, binaries skipped). Returns `(hits, truncated)`;
/// `Err` is a user-facing message shown inside the overlay.
pub fn search(root: &Path, query: &str) -> Result<(Vec<GrepHit>, bool), String> {
    let mut args = vec!["grep", "-z", "-n", "-I", "--untracked", "--no-color", "-F"];
    // Smart case: literal case only when the query has an uppercase char.
    if !query.chars().any(char::is_uppercase) {
        args.push("-i");
    }
    args.extend(["-e", query, "--", "."]);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    // git grep exits 1 for "no matches" — only >= 2 is an error.
    match output.status.code() {
        Some(0) => Ok(parse_grep_z(&output.stdout)),
        Some(1) => Ok((Vec::new(), false)),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("git grep failed: {}", stderr.trim()))
        }
    }
}

/// Parse `git grep -z -n` records: `<path>\0<line>\0<text>\n`, stopping at
/// `MAX_RESULTS` (the bool says whether anything was dropped).
pub fn parse_grep_z(bytes: &[u8]) -> (Vec<GrepHit>, bool) {
    let mut hits = Vec::new();
    for record in bytes.split(|b| *b == b'\n') {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |b| *b == 0);
        let (Some(path), Some(line), Some(text)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(line) = String::from_utf8_lossy(line).parse::<u64>() else {
            continue;
        };
        if hits.len() >= MAX_RESULTS {
            return (hits, true);
        }
        let text = String::from_utf8_lossy(text);
        let text = text.trim();
        hits.push(GrepHit {
            path: String::from_utf8_lossy(path).into_owned(),
            line,
            text: clip(text, MAX_TEXT_LEN),
        });
    }
    (hits, false)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_grep_z_splits_path_line_text() {
        let bytes = b"src/a.rs\x007\x00    let x = grep_me();\nsrc/b file.rs\x0012\x00grep_me\n";
        let (hits, truncated) = parse_grep_z(bytes);
        assert!(!truncated);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "src/a.rs");
        assert_eq!(hits[0].line, 7);
        assert_eq!(hits[0].text, "let x = grep_me();", "text is trimmed");
        assert_eq!(hits[1].path, "src/b file.rs", "spaces in paths survive");
        assert_eq!(hits[1].line, 12);
    }

    #[test]
    fn parse_grep_z_caps_results() {
        let mut bytes = Vec::new();
        for i in 0..(MAX_RESULTS + 5) {
            bytes.extend_from_slice(format!("f.rs\x00{}\x00match\n", i + 1).as_bytes());
        }
        let (hits, truncated) = parse_grep_z(&bytes);
        assert_eq!(hits.len(), MAX_RESULTS);
        assert!(truncated);
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
        std::fs::write(repo.join("tracked.txt"), "needle in a haystack\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        repo
    }

    #[test]
    fn search_finds_tracked_and_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "another Needle here\n").unwrap();

        // Lowercase query is case-insensitive (smart case): both files hit.
        let (hits, truncated) = search(&repo, "needle").unwrap();
        assert!(!truncated);
        assert_eq!(hits.len(), 2, "{hits:?}");
        let tracked = hits.iter().find(|h| h.path == "tracked.txt").unwrap();
        assert_eq!(tracked.line, 1);
        assert_eq!(tracked.text, "needle in a haystack");
        assert!(hits.iter().any(|h| h.path == "fresh.txt"), "{hits:?}");

        // An uppercase char makes the query literal: only fresh.txt hits.
        let (hits, _) = search(&repo, "Needle").unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "fresh.txt");

        // No matches is Ok(empty), not an error (exit code 1).
        let (hits, _) = search(&repo, "no_such_string_zzz").unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn search_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = search(dir.path(), "anything").unwrap_err();
        assert!(err.contains("git grep failed"), "{err}");
    }
}
