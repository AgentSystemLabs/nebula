//! The DIFF VIEWER's file list as a directory tree (`Ctrl+t`).
//!
//! The flat list names every changed file by its whole path, which is what
//! a fuzzy filter wants and what the reader of a forty-file pull request
//! does not: the shape of the change — which crates, which directories —
//! is smeared across forty prefixes. This is the same list folded the way
//! the TREE BROWSER folds a checkout (its node arena and row walk are
//! reused as they are), with two differences a diff earns:
//!
//! - every directory starts open — the list is already only what changed,
//!   and a tree that has to be unfolded before it shows a file is a worse
//!   flat list;
//! - a chain of directories that hold nothing but one another reads as one
//!   row (`crates/nebula-tui/src`), so a change three levels down doesn't
//!   cost three rows and six columns before its first file.
//!
//! The tree only ever says which row is selected; reading that row's diff
//! stays with the caller, as it does for the flat list (`DiffView::select`
//! and friends return whether the selection changed).

use crate::app::clamp_selection;
use crate::git_diff::DiffFile;
use crate::tree_browser::{build_nodes, visible_rows, TreeNode, TreeRow};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct DiffTree {
    pub nodes: Vec<TreeNode>,
    /// Top-level node indices (children of the implicit root).
    pub top: Vec<usize>,
    /// Per-node expansion, honored only while the filter is empty — a live
    /// filter force-expands every hierarchy it keeps.
    pub expanded: Vec<bool>,
    /// File node → its index in `DiffView::files`; `None` for a directory.
    pub file_of: Vec<Option<usize>>,
    /// Visible rows in tree order.
    pub rows: Vec<TreeRow>,
    /// Row of the best-scoring file under a live filter, where a filter
    /// edit parks the selection.
    pub best_row: Option<usize>,
    /// Files matching the filter, for the title.
    pub match_count: usize,
    /// Index into `rows`.
    pub selected: usize,
}

impl DiffTree {
    /// The tree of `files`, everything open, narrowed by `filter`.
    pub fn new(files: &[DiffFile], filter: &str) -> Self {
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let (mut nodes, top, _) = build_nodes(&paths);
        compact_chains(&mut nodes, &top);
        let index: HashMap<&str, usize> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect();
        let file_of = nodes
            .iter()
            .map(|n| {
                (!n.is_dir)
                    .then(|| index.get(n.path.as_str()).copied())
                    .flatten()
            })
            .collect();
        let mut tree = Self {
            expanded: vec![true; nodes.len()],
            nodes,
            top,
            file_of,
            rows: Vec::new(),
            best_row: None,
            match_count: files.len(),
            selected: 0,
        };
        tree.rebuild_rows(filter);
        tree.selected = tree.home_row();
        tree
    }

    /// The same tree over a new file list (a pull request's diff refreshed
    /// underneath the modal): what the reader had folded stays folded, by
    /// path. The cursor goes home (`home_row`); the caller moves it back.
    pub fn rebuilt(&self, files: &[DiffFile], filter: &str) -> Self {
        let folded: HashSet<&str> = self
            .nodes
            .iter()
            .zip(&self.expanded)
            .filter(|(n, open)| n.is_dir && !**open)
            .map(|(n, _)| n.path.as_str())
            .collect();
        let mut tree = Self::new(files, "");
        for (node, open) in tree.nodes.iter().zip(tree.expanded.iter_mut()) {
            if node.is_dir && folded.contains(node.path.as_str()) {
                *open = false;
            }
        }
        tree.rebuild_rows(filter);
        tree.selected = tree.home_row();
        tree
    }

    fn rebuild_rows(&mut self, filter: &str) {
        let visible = visible_rows(&self.nodes, &self.top, &self.expanded, filter);
        self.rows = visible.rows;
        self.best_row = visible.best_row;
        let files = self.file_of.iter().flatten().count();
        self.match_count = visible.match_count.unwrap_or(files);
    }

    /// Where the cursor goes when nothing says otherwise: the best match
    /// under a live filter, else the first file's row — directories sort
    /// first, so the top row is nearly always one, and this modal is for
    /// reading diffs, not folder summaries. The top row when every file is
    /// folded away.
    pub(crate) fn home_row(&self) -> usize {
        self.best_row
            .or_else(|| {
                self.rows
                    .iter()
                    .position(|r| self.file_of[r.node].is_some())
            })
            .unwrap_or(0)
    }

    fn selected_index(&self) -> Option<usize> {
        self.rows.get(self.selected).map(|r| r.node)
    }

    /// The node behind the current selection, if any row is visible.
    pub fn selected_node(&self) -> Option<&TreeNode> {
        self.nodes.get(self.selected_index()?)
    }

    /// The selected row's index in `DiffView::files`; `None` on a
    /// directory row.
    pub fn selected_file(&self) -> Option<usize> {
        self.file_of[self.selected_index()?]
    }

    /// Whether a directory row shows its children: its own fold while the
    /// filter is empty, always under a live one.
    pub fn is_open(&self, node: usize, filter_live: bool) -> bool {
        filter_live || self.expanded[node]
    }

    /// Clamped absolute selection; true when it landed on another row.
    pub fn select(&mut self, index: i64) -> bool {
        let clamped = clamp_selection(index, self.rows.len());
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// Put the selection on the row for `path` — a file's, or a directory
    /// row's — unfolding whatever hides it. False when no row has it: the
    /// path is gone, or a live filter doesn't keep it.
    pub fn select_path(&mut self, path: &str, filter: &str) -> bool {
        // An absorbed chain link keeps an empty path (`compact_chains`).
        let Some(node) = self
            .nodes
            .iter()
            .position(|n| !n.path.is_empty() && n.path == path)
        else {
            return false;
        };
        if filter.is_empty() {
            let mut folded = false;
            let mut parent = self.nodes[node].parent;
            while let Some(p) = parent {
                folded |= !std::mem::replace(&mut self.expanded[p], true);
                parent = self.nodes[p].parent;
            }
            if folded {
                self.rebuild_rows(filter);
            }
        }
        match self.rows.iter().position(|r| r.node == node) {
            Some(row) => {
                self.selected = row;
                true
            }
            None => false,
        }
    }

    /// Recompute `rows` after a filter edit and send the cursor home
    /// (`home_row`); true when that moved it onto another node.
    pub fn apply_filter(&mut self, filter: &str) -> bool {
        let before = self.selected_index();
        self.rebuild_rows(filter);
        self.selected = self.home_row();
        self.selected_index() != before
    }

    /// Enter/click on a directory row: flip its fold, keeping the selection
    /// on the node it was on — one that sat inside the folded subtree falls
    /// back to the directory. No-op on files and under a live filter (the
    /// filtered tree is forced open). True when the selection changed node.
    pub fn toggle_row(&mut self, row: usize, filter: &str) -> bool {
        if !filter.is_empty() {
            return false;
        }
        let Some(node) = self.rows.get(row).map(|r| r.node) else {
            return false;
        };
        if !self.nodes[node].is_dir {
            return false;
        }
        let before = self.selected_index();
        self.expanded[node] = !self.expanded[node];
        self.rebuild_rows(filter);
        self.selected = before
            .and_then(|n| self.rows.iter().position(|r| r.node == n))
            .or_else(|| self.rows.iter().position(|r| r.node == node))
            .unwrap_or(0);
        self.selected_index() != before
    }

    /// → opens a folded directory; on an open one it steps onto the first
    /// child. True when the selection changed node.
    pub fn expand_selected(&mut self, filter: &str) -> bool {
        let Some(node) = self.selected_index() else {
            return false;
        };
        if !self.nodes[node].is_dir {
            return false;
        }
        if self.is_open(node, !filter.is_empty()) {
            return !self.nodes[node].children.is_empty() && self.select(self.selected as i64 + 1);
        }
        self.expanded[node] = true;
        // Only rows below the selection changed; it still points at `node`.
        self.rebuild_rows(filter);
        false
    }

    /// ← folds an open directory; anywhere else it jumps to the parent row.
    /// True when the selection changed node.
    pub fn collapse_selected(&mut self, filter: &str) -> bool {
        let Some(node) = self.selected_index() else {
            return false;
        };
        if filter.is_empty() && self.nodes[node].is_dir && self.expanded[node] {
            self.expanded[node] = false;
            // Only rows below the selection changed; it still points at `node`.
            self.rebuild_rows(filter);
            return false;
        }
        let parent_row = self.nodes[node]
            .parent
            .and_then(|p| self.rows.iter().position(|r| r.node == p));
        parent_row.is_some_and(|row| self.select(row as i64))
    }

    /// Every file under `node`, as indices into `DiffView::files`, in tree
    /// order (`node` itself when it is a file).
    pub fn files_under(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(i) = stack.pop() {
            out.extend(self.file_of[i]);
            stack.extend(self.nodes[i].children.iter().rev());
        }
        out
    }

    /// Per node: a directory whose every file is reviewed ✓ (a file node
    /// says whether it is itself). One bottom-up pass — a node is always
    /// created before its children, so walking the arena backwards has each
    /// directory's children settled before it is asked.
    pub fn reviewed_nodes(&self, files: &[DiffFile], reviewed: &HashMap<String, u64>) -> Vec<bool> {
        let mut done = vec![false; self.nodes.len()];
        for i in (0..self.nodes.len()).rev() {
            done[i] = match self.file_of[i] {
                Some(f) => reviewed.contains_key(&files[f].path),
                None => self.nodes[i].children.iter().all(|&c| done[c]),
            };
        }
        done
    }
}

/// Fold every chain of directories that hold nothing but one another into
/// its head — `crates` → `nebula-tui` → `src` becomes the one row
/// `crates/nebula-tui/src` — and re-count the depths to match. An absorbed
/// link stays in the arena, unreachable, its name and path emptied so no
/// lookup by path can land on it.
fn compact_chains(nodes: &mut [TreeNode], top: &[usize]) {
    let mut stack: Vec<(usize, usize)> = top.iter().map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        nodes[i].depth = depth;
        loop {
            let only = match nodes[i].children.as_slice() {
                &[only] if nodes[only].is_dir => only,
                _ => break,
            };
            let link = std::mem::take(&mut nodes[only].name);
            nodes[i].name = format!("{}/{link}", nodes[i].name);
            nodes[i].path = std::mem::take(&mut nodes[only].path);
            nodes[i].children = std::mem::take(&mut nodes[only].children);
        }
        for c in nodes[i].children.clone() {
            nodes[c].parent = Some(i);
            stack.push((c, depth + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<DiffFile> {
        paths
            .iter()
            .map(|p| DiffFile {
                path: p.to_string(),
                orig_path: None,
                xy: ['M', ' '],
            })
            .collect()
    }

    /// Each visible row as `depth:name`, the way the list draws it.
    fn shown(tree: &DiffTree) -> Vec<String> {
        tree.rows
            .iter()
            .map(|r| format!("{}:{}", tree.nodes[r.node].depth, tree.nodes[r.node].name))
            .collect()
    }

    #[test]
    fn opens_unfolded_with_single_child_chains_as_one_row() {
        let tree = DiffTree::new(
            &files(&[
                "README.md",
                "crates/tui/src/app.rs",
                "crates/tui/src/ui.rs",
                "crates/tui/Cargo.toml",
                "docs/guide/keys.md",
            ]),
            "",
        );
        assert_eq!(
            shown(&tree),
            vec![
                // `crates` holds only `tui`, so the two are one row; `tui`
                // holds a file beside `src`, so the chain stops there.
                "0:crates/tui",
                "1:src",
                "2:app.rs",
                "2:ui.rs",
                "1:Cargo.toml",
                "0:docs/guide",
                "1:keys.md",
                "0:README.md",
            ]
        );
        // A chain's row answers to the deepest directory's path.
        assert_eq!(tree.nodes[tree.rows[0].node].path, "crates/tui");
        assert_eq!(tree.match_count, 5);
    }

    #[test]
    fn every_file_row_maps_back_to_its_diff_file() {
        let list = files(&["b/two.rs", "a/one.rs", "top.rs"]);
        let tree = DiffTree::new(&list, "");
        for row in &tree.rows {
            let node = &tree.nodes[row.node];
            match tree.file_of[row.node] {
                Some(f) => assert_eq!(list[f].path, node.path),
                None => assert!(node.is_dir, "{} has no file", node.path),
            }
        }
        assert_eq!(tree.file_of.iter().flatten().count(), 3);
    }

    #[test]
    fn folding_a_directory_hides_its_files_and_keeps_the_cursor_in_sight() {
        let mut tree = DiffTree::new(&files(&["a/x.rs", "a/y.rs", "b.rs"]), "");
        assert!(tree.select_path("a/y.rs", ""));
        // Folding `a` from inside it: the cursor falls back onto `a`.
        assert!(tree.toggle_row(0, ""));
        assert_eq!(shown(&tree), vec!["0:a", "0:b.rs"]);
        assert_eq!(tree.selected_node().unwrap().path, "a");
        assert_eq!(tree.selected_file(), None);
        // → opens it again without moving; a second → steps inside.
        assert!(!tree.expand_selected(""));
        assert_eq!(shown(&tree), vec!["0:a", "1:x.rs", "1:y.rs", "0:b.rs"]);
        assert!(tree.expand_selected(""));
        assert_eq!(tree.selected_node().unwrap().path, "a/x.rs");
        // ← on a file jumps to its directory; ← there folds it.
        assert!(tree.collapse_selected(""));
        assert_eq!(tree.selected_node().unwrap().path, "a");
        assert!(!tree.collapse_selected(""));
        assert_eq!(shown(&tree), vec!["0:a", "0:b.rs"]);
    }

    #[test]
    fn select_path_unfolds_what_hides_the_file() {
        let mut tree = DiffTree::new(&files(&["a/sub/x.rs", "a/y.rs"]), "");
        tree.toggle_row(0, "");
        assert_eq!(shown(&tree), vec!["0:a"]);
        assert!(tree.select_path("a/sub/x.rs", ""));
        assert_eq!(tree.selected_node().unwrap().path, "a/sub/x.rs");
        assert!(!tree.select_path("gone.rs", ""));
    }

    #[test]
    fn a_filter_keeps_matching_files_under_their_directories_forced_open() {
        let mut tree = DiffTree::new(&files(&["a/sub/z.rs", "a/x.rs", "other/w.rs"]), "");
        tree.toggle_row(0, ""); // fold `a`
        assert!(tree.apply_filter("z"));
        assert_eq!(shown(&tree), vec!["0:a", "1:sub", "2:z.rs"]);
        assert_eq!(tree.match_count, 1);
        // The cursor parks on the match, not on the hierarchy above it.
        assert_eq!(tree.selected_node().unwrap().path, "a/sub/z.rs");
        // Folding is off while the filter is live.
        assert!(!tree.toggle_row(0, "z"));
        assert_eq!(tree.rows.len(), 3);
        // Clearing it brings the reader's own folds back.
        tree.apply_filter("");
        assert_eq!(shown(&tree), vec!["0:a", "0:other", "1:w.rs"]);
    }

    /// Home is a file's row, not the directory sorted above it — on open,
    /// and again when a filter is cleared.
    #[test]
    fn the_cursor_goes_home_to_the_first_file() {
        let mut tree = DiffTree::new(&files(&["a/x.rs", "a/y.rs", "b.rs"]), "");
        assert_eq!(tree.selected_node().unwrap().path, "a/x.rs");
        tree.apply_filter("b");
        assert_eq!(tree.selected_node().unwrap().path, "b.rs");
        tree.apply_filter("");
        assert_eq!(tree.selected_node().unwrap().path, "a/x.rs");
        // Nothing but folded directories in sight: the top row it is.
        let mut folded = DiffTree::new(&files(&["a/x.rs"]), "");
        folded.toggle_row(0, "");
        folded.apply_filter("");
        assert_eq!(folded.selected_node().unwrap().path, "a");
    }

    #[test]
    fn a_rebuild_keeps_the_readers_folds_by_path() {
        let mut tree = DiffTree::new(&files(&["a/x.rs", "b/y.rs"]), "");
        tree.toggle_row(0, ""); // fold `a`
        let tree = tree.rebuilt(&files(&["a/x.rs", "a/new.rs", "b/y.rs", "c/z.rs"]), "");
        assert_eq!(shown(&tree), vec!["0:a", "0:b", "1:y.rs", "0:c", "1:z.rs"]);
    }

    #[test]
    fn a_directory_is_reviewed_once_every_file_under_it_is() {
        let list = files(&["a/x.rs", "a/sub/y.rs", "b.rs"]);
        let tree = DiffTree::new(&list, "");
        let dir_a = tree.nodes.iter().position(|n| n.path == "a").unwrap();
        let mut reviewed = HashMap::new();
        reviewed.insert("a/x.rs".to_string(), 1);
        assert!(!tree.reviewed_nodes(&list, &reviewed)[dir_a]);
        reviewed.insert("a/sub/y.rs".to_string(), 2);
        assert!(tree.reviewed_nodes(&list, &reviewed)[dir_a]);
        assert_eq!(
            tree.files_under(dir_a)
                .into_iter()
                .map(|f| list[f].path.as_str())
                .collect::<Vec<_>>(),
            vec!["a/sub/y.rs", "a/x.rs"],
            "tree order: directories first"
        );
    }
}
