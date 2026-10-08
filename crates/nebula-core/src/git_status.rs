//! `git status --porcelain=v1 -z`, parsed once for every crate that reads
//! it: the TUI's DIFF VIEWER and changed-file counts, and the daemon's
//! shared-worktree check before `nebula spawn` starts a second session in
//! one checkout.

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

/// Parse NUL-separated porcelain v1: `XY path\0`, and for a rename or copy
/// a second NUL-terminated field holding the ORIGINAL path
/// (`XY new\0old\0`). Either column can say so: `R ` / `C ` for one in the
/// index, ` R` for one git detects in the worktree (an intent-to-add file,
/// `git add -N`, whose content matches a deleted one). Reading only the
/// index column took that second field for an entry of its own — and when
/// the original path itself began with `R` or `C` (`README.md`), for a
/// rename that swallowed the real entry after it.
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
        let orig_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Captured from git 2.39: `mv README.md moved.md && git add -N
    /// moved.md` reports a worktree-side rename, its original path in the
    /// next field — one whose name starts with `R`, which the index-column
    /// test read as a rename entry of its own.
    #[test]
    fn a_worktree_rename_keeps_its_original_path_out_of_the_list() {
        let files = parse_status_z(b" R moved.md\0README.md\0 M src/lib.rs\0");
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(files[0].path, "moved.md");
        assert_eq!(files[0].orig_path.as_deref(), Some("README.md"));
        assert_eq!(files[1].path, "src/lib.rs");
    }
}
