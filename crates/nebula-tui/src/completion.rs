//! Bash-style filesystem path completion for the Add-project prompt.
//!
//! Semantics (matching bash's readline closely enough to feel familiar):
//! - single match → complete it fully and append `/`
//! - multiple matches → extend the input to the longest common prefix and
//!   surface the candidate list for display
//! - `~/…` is expanded for lookup but the tilde is preserved in the input
//! - only directories are offered (projects are git repos)
//! - dotted entries are hidden unless the partial segment starts with `.`

use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq)]
pub struct PathCompletion {
    /// New input value when the completion made progress.
    pub completed: Option<String>,
    /// Candidate directory names (with trailing `/`) when ambiguous.
    pub candidates: Vec<String>,
}

/// One row of the Add-project directory listing.
#[derive(Debug, Clone, PartialEq)]
pub struct DirEntry {
    /// Directory name, no trailing slash.
    pub name: String,
    /// Whether it holds a `.git` entry (dir, or file for linked worktrees).
    pub is_repo: bool,
}

/// Split `input` at its last '/': the typed parent (kept verbatim, tilde
/// and all) and the partial segment being completed.
pub fn split_input(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(idx) => (&input[..=idx], &input[idx + 1..]),
        None => ("", input),
    }
}

/// Live listing behind the Add-project browser: the typed parent's
/// subdirectories narrowed to the partial segment, name-sorted. Same rules
/// as [`complete_path`] (directories only, dotted entries need a dotted
/// partial), plus a git-repo marker per entry.
pub fn list_dirs(input: &str, home: Option<&Path>) -> Vec<DirEntry> {
    let (typed_parent, partial) = split_input(input);
    let parent_dir = expand_parent(typed_parent, home);
    let Ok(entries) = std::fs::read_dir(&parent_dir) else {
        return Vec::new();
    };
    let show_hidden = partial.starts_with('.');
    let mut dirs: Vec<DirEntry> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(partial))
        .filter(|name| show_hidden || !name.starts_with('.'))
        .map(|name| {
            let is_repo = parent_dir.join(&name).join(".git").exists();
            DirEntry { name, is_repo }
        })
        .collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    dirs
}

/// Complete `input` against the filesystem. `home` backs `~` expansion.
pub fn complete_path(input: &str, home: Option<&Path>) -> PathCompletion {
    // Bare "~" → "~/" so the next Tab lists the home directory.
    if input == "~" {
        return PathCompletion {
            completed: Some("~/".into()),
            candidates: vec![],
        };
    }

    let (typed_parent, partial) = split_input(input);

    let parent_dir = expand_parent(typed_parent, home);
    let Ok(entries) = std::fs::read_dir(&parent_dir) else {
        return PathCompletion::default();
    };

    let show_hidden = partial.starts_with('.');
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(partial))
        .filter(|name| show_hidden || !name.starts_with('.'))
        .collect();
    names.sort();

    match names.len() {
        0 => PathCompletion::default(),
        1 => PathCompletion {
            completed: Some(format!("{typed_parent}{}/", names[0])),
            candidates: vec![],
        },
        _ => {
            let lcp = longest_common_prefix(&names);
            let completed = if lcp.len() > partial.len() {
                Some(format!("{typed_parent}{lcp}"))
            } else {
                None
            };
            PathCompletion {
                completed,
                candidates: names.into_iter().map(|n| format!("{n}/")).collect(),
            }
        }
    }
}

/// Expand the typed parent ("", "~/", "~/x/", "/a/b/", "rel/") to a real dir.
fn expand_parent(typed_parent: &str, home: Option<&Path>) -> PathBuf {
    if typed_parent.is_empty() {
        return PathBuf::from(".");
    }
    if let Some(home) = home {
        if typed_parent == "~/" {
            return home.to_path_buf();
        }
        if let Some(rest) = typed_parent.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(typed_parent)
}

fn longest_common_prefix(names: &[String]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut lcp = first.as_str();
    for name in &names[1..] {
        let common_bytes = lcp
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map(|((i, c), _)| i + c.len_utf8())
            .unwrap_or(0);
        lcp = &lcp[..common_bytes];
        if lcp.is_empty() {
            break;
        }
    }
    lcp.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// projects/{alpha,alps,beta,.hidden}/ + a file notes.txt
    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for dir in [
            "projects/alpha",
            "projects/alps",
            "projects/beta",
            "projects/.hidden",
        ] {
            std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        std::fs::write(tmp.path().join("projects/notes.txt"), "").unwrap();
        tmp
    }

    fn p(tmp: &tempfile::TempDir, rest: &str) -> String {
        format!("{}/{rest}", tmp.path().display())
    }

    #[test]
    fn single_match_completes_with_trailing_slash() {
        let tmp = fixture();
        let got = complete_path(&p(&tmp, "proj"), None);
        assert_eq!(got.completed, Some(p(&tmp, "projects/")));
        assert!(got.candidates.is_empty());
    }

    #[test]
    fn unambiguous_nested_match() {
        let tmp = fixture();
        let got = complete_path(&p(&tmp, "projects/be"), None);
        assert_eq!(got.completed, Some(p(&tmp, "projects/beta/")));
    }

    #[test]
    fn ambiguous_extends_to_longest_common_prefix_and_lists() {
        let tmp = fixture();
        let got = complete_path(&p(&tmp, "projects/a"), None);
        assert_eq!(
            got.completed,
            Some(p(&tmp, "projects/alp")),
            "extends a → alp"
        );
        assert_eq!(got.candidates, vec!["alpha/", "alps/"]);
    }

    #[test]
    fn at_lcp_already_just_lists_candidates() {
        let tmp = fixture();
        let got = complete_path(&p(&tmp, "projects/alp"), None);
        assert_eq!(got.completed, None, "no further progress possible");
        assert_eq!(got.candidates, vec!["alpha/", "alps/"]);
    }

    #[test]
    fn files_are_not_offered() {
        let tmp = fixture();
        let got = complete_path(&p(&tmp, "projects/no"), None);
        assert_eq!(
            got,
            PathCompletion::default(),
            "notes.txt is a file, not a project dir"
        );
    }

    #[test]
    fn hidden_dirs_only_with_dot_partial() {
        let tmp = fixture();
        // "projects/" partial "" → hidden excluded, both visible prefixes listed.
        let got = complete_path(&p(&tmp, "projects/"), None);
        assert_eq!(got.candidates, vec!["alpha/", "alps/", "beta/"]);
        // Typing the dot opts in.
        let got = complete_path(&p(&tmp, "projects/."), None);
        assert_eq!(got.completed, Some(p(&tmp, "projects/.hidden/")));
    }

    #[test]
    fn tilde_is_expanded_for_lookup_but_preserved_in_input() {
        let tmp = fixture();
        let got = complete_path("~/proj", Some(tmp.path()));
        assert_eq!(got.completed, Some("~/projects/".into()));
        let got = complete_path("~/projects/be", Some(tmp.path()));
        assert_eq!(got.completed, Some("~/projects/beta/".into()));
    }

    #[test]
    fn bare_tilde_becomes_tilde_slash() {
        let got = complete_path("~", None);
        assert_eq!(got.completed, Some("~/".into()));
    }

    #[test]
    fn nonexistent_parent_is_a_noop() {
        let got = complete_path("/definitely/not/a/real/dir/x", None);
        assert_eq!(got, PathCompletion::default());
    }

    fn names(dirs: &[DirEntry]) -> Vec<&str> {
        dirs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn list_dirs_narrows_by_partial_and_hides_dotted() {
        let tmp = fixture();
        let got = list_dirs(&p(&tmp, "projects/"), None);
        assert_eq!(names(&got), vec!["alpha", "alps", "beta"]);
        let got = list_dirs(&p(&tmp, "projects/al"), None);
        assert_eq!(names(&got), vec!["alpha", "alps"]);
        // Dotted entries need a dotted partial, files never appear.
        let got = list_dirs(&p(&tmp, "projects/."), None);
        assert_eq!(names(&got), vec![".hidden"]);
        let got = list_dirs(&p(&tmp, "projects/no"), None);
        assert!(got.is_empty(), "notes.txt is a file");
    }

    #[test]
    fn list_dirs_marks_git_repos() {
        let tmp = fixture();
        std::fs::create_dir_all(tmp.path().join("projects/alpha/.git")).unwrap();
        // Linked worktrees keep `.git` as a file — still a repo.
        std::fs::write(tmp.path().join("projects/beta/.git"), "gitdir: x").unwrap();
        let got = list_dirs(&p(&tmp, "projects/"), None);
        let repos: Vec<(&str, bool)> = got.iter().map(|d| (d.name.as_str(), d.is_repo)).collect();
        assert_eq!(
            repos,
            vec![("alpha", true), ("alps", false), ("beta", true)]
        );
    }

    #[test]
    fn list_dirs_expands_tilde_and_survives_bad_parents() {
        let tmp = fixture();
        let got = list_dirs("~/projects/al", Some(tmp.path()));
        assert_eq!(names(&got), vec!["alpha", "alps"]);
        assert!(list_dirs("/definitely/not/a/real/dir/", None).is_empty());
    }
}
