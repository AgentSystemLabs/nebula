//! Full-worktree tree browser (`t`): directory tree left, file preview
//! right, with an always-live fuzzy filter that narrows the tree to the
//! matching files and the hierarchies containing them.

use crate::app::{DEFAULT_DIFF_FILES_W, MIN_DIFF_FILES_W, MIN_DIFF_PANE_W};
use crate::syntax::{Highlighter, TokenKind};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;

/// Keep pathological files from bloating the preview state.
const MAX_PREVIEW_LINES: usize = 10_000;
const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

/// One node of the file tree, built once from the git listing.
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// Last path segment, shown in the tree.
    pub name: String,
    /// Full path relative to the worktree root.
    pub path: String,
    pub parent: Option<usize>,
    /// Child node indices: directories first, then files, each name-sorted.
    pub children: Vec<usize>,
    pub is_dir: bool,
    /// Nesting depth; top-level entries are 0.
    pub depth: usize,
}

/// One visible row: a node index plus the char positions of the node's
/// `name` the filter matched, for highlighting.
#[derive(Debug, Clone)]
pub struct TreeRow {
    pub node: usize,
    pub positions: Vec<usize>,
}

/// Full-screen tree browser: file tree left, scrollable preview right.
#[derive(Debug, Clone)]
pub struct TreeBrowser {
    /// Checkout dir the listing was read from.
    pub root: PathBuf,
    /// Branch name for the pane title.
    pub branch: String,
    /// Editor command Enter launches (NEBULA_EDITOR, default vim), captured
    /// at open time.
    pub editor: String,
    pub nodes: Vec<TreeNode>,
    /// Top-level node indices (children of the implicit root).
    pub top: Vec<usize>,
    /// Per-node expansion, honored only while the filter is empty — a live
    /// filter force-expands every hierarchy it keeps.
    pub expanded: Vec<bool>,
    /// Type-to-filter query over file paths; always live.
    pub filter: String,
    /// Total file (non-directory) count, for the title.
    pub file_count: usize,
    /// Files matching the filter, for the title.
    pub match_count: usize,
    /// Visible rows in tree order: everything expanded when the filter is
    /// empty, else the matching files plus their ancestor directories.
    pub rows: Vec<TreeRow>,
    /// Row of the best-scoring file under a live filter, where a filter
    /// edit parks the selection.
    pub best_row: Option<usize>,
    /// Index into `rows`.
    pub selected: usize,
    /// Preview text of the selected node (reloaded on selection change):
    /// file contents, or a child listing for a directory.
    pub preview: String,
    /// `preview` split into syntax-highlighted (kind, text) runs per line;
    /// styling itself lives in ui.rs (the `classify_diff_line` rule).
    pub preview_lines: Vec<Vec<(TokenKind, String)>>,
    /// Cached line count of `preview`, for scroll clamping.
    pub preview_line_count: usize,
    /// Top visible preview line.
    pub scroll: u16,
    /// Inner height of the preview pane, written back during draw (the
    /// `DiffView::view_height` pattern) so paging tracks resizes.
    pub view_height: u16,
    /// Screen rect of the tree rows (filter row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// Inner rect of the preview pane, written back during draw; the
    /// embedded editor spawns and renders at this size.
    pub preview_area: Rect,
    /// Full modal rect, written back during draw; bounds the tree-panel
    /// splitter drag and hit-tests its border.
    pub area: Rect,
    /// Outer width of the tree panel; drag the panel border to resize.
    pub files_width: u16,
    /// In-progress drag of the tree/preview border: `boundary_x - grab
    /// column` at mouse-down (the `SplitterDrag::grab_offset` pattern).
    pub files_drag: Option<i32>,
}

impl TreeBrowser {
    pub fn new(root: PathBuf, branch: String, editor: String, files: Vec<String>) -> Self {
        let (nodes, top, file_count) = build_nodes(&files);
        let expanded = vec![false; nodes.len()];
        let mut browser = Self {
            root,
            branch,
            editor,
            nodes,
            top,
            expanded,
            filter: String::new(),
            file_count,
            match_count: file_count,
            rows: Vec::new(),
            best_row: None,
            selected: 0,
            preview: String::new(),
            preview_lines: Vec::new(),
            preview_line_count: 0,
            scroll: 0,
            view_height: 0,
            list_area: Rect::default(),
            preview_area: Rect::default(),
            area: Rect::default(),
            files_width: DEFAULT_DIFF_FILES_W,
            files_drag: None,
        };
        browser.rebuild_rows();
        browser.load_preview();
        browser
    }

    pub fn max_scroll(&self) -> u16 {
        (self.preview_line_count as u16).saturating_sub(self.view_height.max(1))
    }

    /// Clamped relative preview scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32) as u16;
    }

    /// Screen x of the tree/preview boundary — the column where the preview
    /// panel starts.
    pub fn splitter_x(&self) -> u16 {
        self.area.x + self.files_width
    }

    /// Move the tree/preview boundary to `boundary_x`, clamped so the tree
    /// keeps `MIN_DIFF_FILES_W` and the preview keeps `MIN_DIFF_PANE_W`.
    pub fn set_files_width(&mut self, boundary_x: i32) {
        let max = self.area.width.saturating_sub(MIN_DIFF_PANE_W);
        if max < MIN_DIFF_FILES_W {
            return; // modal too small to honor the minimums
        }
        let want = (boundary_x - self.area.x as i32).max(0) as u16;
        self.files_width = want.clamp(MIN_DIFF_FILES_W, max);
    }

    /// First visible row of the tree's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        (self.selected + 1).saturating_sub(height)
    }

    /// Clamped absolute selection; reloads the preview when it moved.
    pub fn select(&mut self, index: i64) {
        let max = self.rows.len().saturating_sub(1) as i64;
        let clamped = index.clamp(0, max) as usize;
        if clamped != self.selected {
            self.selected = clamped;
            self.load_preview();
        }
    }

    /// The node behind the current selection, if any row is visible.
    pub fn selected_node(&self) -> Option<&TreeNode> {
        self.nodes.get(self.rows.get(self.selected)?.node)
    }

    pub fn selected_is_dir(&self) -> bool {
        self.selected_node().is_some_and(|n| n.is_dir)
    }

    /// Enter/click on a directory row: flip its expansion, keeping the
    /// selection on that row. No-op on files and under a live filter (the
    /// filtered tree is forced open).
    pub fn toggle_row(&mut self, row: usize) {
        if !self.filter.is_empty() {
            return;
        }
        let Some(node) = self.rows.get(row).map(|r| r.node) else {
            return;
        };
        if !self.nodes[node].is_dir {
            return;
        }
        let before = self.rows.get(self.selected).map(|r| r.node);
        self.expanded[node] = !self.expanded[node];
        self.rebuild_rows();
        // Keep the selection on the node it was on; a selection that sat
        // inside the folded subtree falls back to the toggled directory.
        self.selected = before
            .and_then(|n| self.rows.iter().position(|r| r.node == n))
            .or_else(|| self.rows.iter().position(|r| r.node == node))
            .unwrap_or(0);
        if self.rows.get(self.selected).map(|r| r.node) != before {
            self.load_preview();
        }
    }

    /// → expands a collapsed dir; on an expanded dir it steps into the
    /// first child.
    pub fn expand_selected(&mut self) {
        let Some(node) = self.rows.get(self.selected).map(|r| r.node) else {
            return;
        };
        if !self.nodes[node].is_dir {
            return;
        }
        if !self.filter.is_empty() || self.expanded[node] {
            // Already open (a live filter forces every dir open): the first
            // child is the next row.
            if !self.nodes[node].children.is_empty() {
                self.select(self.selected as i64 + 1);
            }
            return;
        }
        self.expanded[node] = true;
        // Only rows below the selection changed; it still points at `node`.
        self.rebuild_rows();
    }

    /// ← collapses an expanded dir; anywhere else it jumps to the parent row.
    pub fn collapse_selected(&mut self) {
        let Some(node) = self.rows.get(self.selected).map(|r| r.node) else {
            return;
        };
        if self.filter.is_empty() && self.nodes[node].is_dir && self.expanded[node] {
            self.expanded[node] = false;
            // Only rows below the selection changed; it still points at `node`.
            self.rebuild_rows();
            return;
        }
        if let Some(parent) = self.nodes[node].parent {
            if let Some(row) = self.rows.iter().position(|r| r.node == parent) {
                self.select(row as i64);
            }
        }
    }

    /// Recompute `rows` after a filter edit and park the selection on the
    /// best-scoring file (top row when the filter is empty), reloading the
    /// preview when the selected node changed.
    pub fn apply_filter(&mut self) {
        let before = self.rows.get(self.selected).map(|r| r.node);
        self.rebuild_rows();
        self.selected = self
            .best_row
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        if self.rows.get(self.selected).map(|r| r.node) != before {
            self.load_preview();
        }
    }

    /// Recompute `rows` from `filter` + `expanded`. An empty filter walks
    /// the expansion state; otherwise every file whose path fuzzy-matches is
    /// kept along with its ancestor directories, all forced open.
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        self.best_row = None;
        if self.filter.is_empty() {
            self.match_count = self.file_count;
            let mut stack: Vec<usize> = self.top.iter().rev().copied().collect();
            while let Some(i) = stack.pop() {
                self.rows.push(TreeRow {
                    node: i,
                    positions: Vec::new(),
                });
                if self.nodes[i].is_dir && self.expanded[i] {
                    for &c in self.nodes[i].children.iter().rev() {
                        stack.push(c);
                    }
                }
            }
            return;
        }
        // Match files on their full relative path (the file-finder rule, so
        // "tui/app" works), then include every ancestor directory.
        let mut include = vec![false; self.nodes.len()];
        let mut name_positions: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut scores: Vec<Option<i32>> = vec![None; self.nodes.len()];
        self.match_count = 0;
        for i in 0..self.nodes.len() {
            if self.nodes[i].is_dir {
                continue;
            }
            let Some(m) = crate::fuzzy::fuzzy_match(&self.filter, &self.nodes[i].path) else {
                continue;
            };
            self.match_count += 1;
            // Map full-path match positions onto the displayed name; a hit
            // landing in the directory prefix lights nothing on this row.
            let name_chars = self.nodes[i].name.chars().count();
            let offset = self.nodes[i].path.chars().count() - name_chars;
            name_positions[i] = m
                .positions
                .iter()
                .filter(|&&p| p >= offset)
                .map(|p| p - offset)
                .collect();
            scores[i] = Some(m.score);
            include[i] = true;
            let mut parent = self.nodes[i].parent;
            while let Some(p) = parent {
                if include[p] {
                    break;
                }
                include[p] = true;
                parent = self.nodes[p].parent;
            }
        }
        let mut best: Option<(i32, usize)> = None;
        let mut stack: Vec<usize> = self.top.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            if !include[i] {
                continue;
            }
            let row = self.rows.len();
            self.rows.push(TreeRow {
                node: i,
                positions: std::mem::take(&mut name_positions[i]),
            });
            if let Some(score) = scores[i] {
                if best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, row));
                }
            }
            if self.nodes[i].is_dir {
                for &c in self.nodes[i].children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        self.best_row = best.map(|(_, row)| row);
    }

    /// Reload the preview for the current selection and reset the scroll.
    /// Never fails: errors become the displayed text (the `diff_for` rule).
    /// Real file contents get syntax-highlighted; directory listings and
    /// placeholder messages stay plain.
    pub fn load_preview(&mut self) {
        self.scroll = 0;
        let (text, highlight_path) = match self.selected_node() {
            Some(n) if n.is_dir => {
                let listing = n
                    .children
                    .iter()
                    .map(|&c| {
                        let child = &self.nodes[c];
                        if child.is_dir {
                            format!("{}/", child.name)
                        } else {
                            child.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (listing, None)
            }
            Some(n) => match read_preview(&self.root.join(&n.path)) {
                Ok(text) => (text, Some(n.path.clone())),
                Err(message) => (message, None),
            },
            None => (String::new(), None),
        };
        let mut hl = match &highlight_path {
            Some(path) => Highlighter::for_path(path),
            None => Highlighter::plain(),
        };
        self.preview_lines = text.lines().map(|l| hl.line(l)).collect();
        self.preview_line_count = self.preview_lines.len();
        self.preview = text;
    }
}

/// File contents for the preview pane, capped and binary-guarded. `Err` is
/// the placeholder/error message to display (unhighlighted).
fn read_preview(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Err(format!("couldn't read file: {e}")),
    };
    let mut bytes = Vec::new();
    if let Err(e) = file.take((MAX_PREVIEW_BYTES + 1) as u64).read_to_end(&mut bytes) {
        return Err(format!("couldn't read file: {e}"));
    }
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("(binary file)".to_string());
    }
    let byte_capped = bytes.len() > MAX_PREVIEW_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PREVIEW_BYTES)]);
    if text.trim().is_empty() {
        return Err("(empty file)".to_string());
    }
    let mut lines = text.lines();
    let mut out: String = lines
        .by_ref()
        .take(MAX_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if lines.next().is_some() || byte_capped {
        out.push_str("\n… (truncated)");
    }
    // ratatui doesn't expand tabs; keep columns readable.
    Ok(out.replace('\t', "    "))
}

/// Build the node arena from the git listing: directories are implied by
/// the paths, children sorted directories-first then by name.
fn build_nodes(files: &[String]) -> (Vec<TreeNode>, Vec<usize>, usize) {
    let mut nodes: Vec<TreeNode> = Vec::new();
    let mut top: Vec<usize> = Vec::new();
    let mut dir_index: HashMap<String, usize> = HashMap::new();
    let mut file_count = 0;
    for path in files {
        if path.is_empty() {
            continue;
        }
        let segments: Vec<&str> = path.split('/').collect();
        let mut parent: Option<usize> = None;
        let mut prefix = String::new();
        for (depth, segment) in segments.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            if depth + 1 == segments.len() {
                let index = nodes.len();
                nodes.push(TreeNode {
                    name: (*segment).to_string(),
                    path: prefix.clone(),
                    parent,
                    children: Vec::new(),
                    is_dir: false,
                    depth,
                });
                match parent {
                    Some(p) => nodes[p].children.push(index),
                    None => top.push(index),
                }
                file_count += 1;
            } else {
                let index = match dir_index.get(&prefix) {
                    Some(&i) => i,
                    None => {
                        let index = nodes.len();
                        nodes.push(TreeNode {
                            name: (*segment).to_string(),
                            path: prefix.clone(),
                            parent,
                            children: Vec::new(),
                            is_dir: true,
                            depth,
                        });
                        match parent {
                            Some(p) => nodes[p].children.push(index),
                            None => top.push(index),
                        }
                        dir_index.insert(prefix.clone(), index);
                        index
                    }
                };
                parent = Some(index);
            }
        }
    }
    let by_kind_then_name = |nodes: &[TreeNode], a: &usize, b: &usize| {
        (!nodes[*a].is_dir, &nodes[*a].name).cmp(&(!nodes[*b].is_dir, &nodes[*b].name))
    };
    for i in 0..nodes.len() {
        let mut children = std::mem::take(&mut nodes[i].children);
        children.sort_by(|a, b| by_kind_then_name(&nodes, a, b));
        nodes[i].children = children;
    }
    top.sort_by(|a, b| by_kind_then_name(&nodes, a, b));
    (nodes, top, file_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser(files: &[&str]) -> TreeBrowser {
        TreeBrowser::new(
            "/nonexistent-nebula-tree-test".into(),
            "main".into(),
            "vim".into(),
            files.iter().map(|f| f.to_string()).collect(),
        )
    }

    fn visible_paths(b: &TreeBrowser) -> Vec<&str> {
        b.rows
            .iter()
            .map(|r| b.nodes[r.node].path.as_str())
            .collect()
    }

    #[test]
    fn builds_collapsed_tree_dirs_first() {
        let b = browser(&["b.txt", "a/x.rs", "a/y.rs", "a/sub/z.rs"]);
        assert_eq!(b.file_count, 4);
        // Collapsed by default: only top-level rows, dir before file.
        assert_eq!(visible_paths(&b), vec!["a", "b.txt"]);
        assert!(b.nodes[b.rows[0].node].is_dir);
    }

    #[test]
    fn expand_and_collapse_walk_the_hierarchy() {
        let mut b = browser(&["b.txt", "a/x.rs", "a/sub/z.rs"]);
        b.expand_selected(); // open "a": sub/ before x.rs
        assert_eq!(visible_paths(&b), vec!["a", "a/sub", "a/x.rs", "b.txt"]);
        b.expand_selected(); // already open: step into first child
        assert_eq!(b.selected_node().unwrap().path, "a/sub");
        b.expand_selected();
        assert_eq!(
            visible_paths(&b),
            vec!["a", "a/sub", "a/sub/z.rs", "a/x.rs", "b.txt"]
        );
        b.select(2); // land on z.rs
        assert_eq!(b.selected_node().unwrap().path, "a/sub/z.rs");
        b.collapse_selected(); // file: jump to parent dir
        assert_eq!(b.selected_node().unwrap().path, "a/sub");
        b.collapse_selected(); // expanded dir: fold it
        assert_eq!(visible_paths(&b), vec!["a", "a/sub", "a/x.rs", "b.txt"]);
        b.collapse_selected(); // collapsed dir: jump to parent
        assert_eq!(b.selected_node().unwrap().path, "a");
    }

    #[test]
    fn toggle_row_pulls_selection_out_of_a_folded_subtree() {
        let mut b = browser(&["a/x.rs", "a/y.rs"]);
        b.expand_selected();
        b.select(2); // a/y.rs
        b.toggle_row(0); // fold "a" while the selection sits inside it
        assert_eq!(visible_paths(&b), vec!["a"]);
        assert_eq!(b.selected_node().unwrap().path, "a");
    }

    #[test]
    fn filter_keeps_matching_files_and_their_hierarchies() {
        let mut b = browser(&["a/sub/z.rs", "a/x.rs", "other/w.rs"]);
        b.filter = "z".into();
        b.apply_filter();
        // Only z.rs matches; its ancestors appear, "other" does not.
        assert_eq!(visible_paths(&b), vec!["a", "a/sub", "a/sub/z.rs"]);
        assert_eq!(b.match_count, 1);
        // The selection parks on the matching file, not a hierarchy row.
        assert_eq!(b.selected_node().unwrap().path, "a/sub/z.rs");
    }

    #[test]
    fn filter_matches_full_paths_and_highlights_the_name() {
        let mut b = browser(&["a/sub/z.rs", "a/x.rs"]);
        b.filter = "az".into(); // 'a' hits the dir prefix, 'z' hits the name
        b.apply_filter();
        assert_eq!(b.match_count, 1);
        let row = b.rows.iter().find(|r| !b.nodes[r.node].is_dir).unwrap();
        assert_eq!(b.nodes[row.node].path, "a/sub/z.rs");
        // Only the in-name hit ('z' at char 0 of "z.rs") is lit.
        assert_eq!(row.positions, vec![0]);
    }

    #[test]
    fn clearing_the_filter_restores_the_expansion_state() {
        let mut b = browser(&["a/x.rs", "b.txt"]);
        b.filter = "rs".into();
        b.apply_filter();
        assert_eq!(visible_paths(&b), vec!["a", "a/x.rs"]);
        b.filter.clear();
        b.apply_filter();
        // "a" was never manually expanded, so it folds back up.
        assert_eq!(visible_paths(&b), vec!["a", "b.txt"]);
    }

    #[test]
    fn dir_preview_lists_children() {
        let b = browser(&["a/x.rs", "a/sub/z.rs"]);
        assert_eq!(b.preview, "sub/\nx.rs");
    }

    #[test]
    fn file_preview_reads_content_and_survives_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "hello\nworld\n").unwrap();
        let mut b = TreeBrowser::new(
            dir.path().to_path_buf(),
            "main".into(),
            "vim".into(),
            vec!["src/lib.rs".into(), "gone.txt".into()],
        );
        b.filter = "lib".into();
        b.apply_filter();
        assert_eq!(b.preview, "hello\nworld");
        assert_eq!(b.preview_line_count, 2);
        b.filter = "gone".into();
        b.apply_filter();
        assert!(b.preview.starts_with("couldn't read file:"), "{}", b.preview);
    }

    #[test]
    fn file_preview_is_syntax_highlighted_but_placeholders_stay_plain() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "let x = 1; // hi\n").unwrap();
        let mut b = TreeBrowser::new(
            dir.path().to_path_buf(),
            "main".into(),
            "vim".into(),
            vec!["main.rs".into()],
        );
        assert_eq!(
            b.preview_lines,
            vec![vec![
                (TokenKind::Keyword, "let".to_string()),
                (TokenKind::Text, " x = ".to_string()),
                (TokenKind::Number, "1".to_string()),
                (TokenKind::Text, "; ".to_string()),
                (TokenKind::Comment, "// hi".to_string()),
            ]]
        );
        // A read failure is a plain message, never fed to the highlighter.
        std::fs::remove_file(dir.path().join("main.rs")).unwrap();
        b.load_preview();
        assert_eq!(b.preview_lines.len(), 1);
        assert!(
            b.preview_lines[0]
                .iter()
                .all(|(k, _)| *k == TokenKind::Text),
            "{:?}",
            b.preview_lines
        );
    }

    #[test]
    fn binary_files_show_a_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.bin"), b"\x00\x01\x02").unwrap();
        let b = TreeBrowser::new(
            dir.path().to_path_buf(),
            "main".into(),
            "vim".into(),
            vec!["blob.bin".into()],
        );
        assert_eq!(b.preview, "(binary file)");
    }
}
