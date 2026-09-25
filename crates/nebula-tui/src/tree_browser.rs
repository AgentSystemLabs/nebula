//! Full-worktree tree browser (`b`): directory tree left, file preview
//! right, with an always-live fuzzy filter that narrows the tree to the
//! matching files and the hierarchies containing them.

use crate::app::{
    clamp_files_width, clamp_selection, max_scroll, scrolled_by, window_start, DEFAULT_DIFF_FILES_W,
};
use crate::git_diff::cap_lines;
use crate::markdown::{self, Rendered};
use crate::syntax::{Highlighter, TokenKind};
use crate::text_input::TextInput;
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

/// What the preview pane shows for one node, ready to draw: the text, its
/// syntax-highlighted lines, and what kind of thing it is. Reading a file
/// and highlighting it is the slow half of walking the tree — 56 ms for a
/// megabyte of Rust under the INPUT LATENCY PROBE — so a browser with
/// BACKGROUND READS builds this off the loop ([`file_preview`]).
#[derive(Debug, Clone, Default)]
pub struct Preview {
    pub text: String,
    pub lines: Vec<Vec<(TokenKind, String)>>,
    /// Real file contents (earns a line-number gutter), as opposed to a
    /// directory listing or a placeholder message.
    pub is_file: bool,
    pub markdown: bool,
}

impl Preview {
    /// Unhighlighted text: a directory listing, an error, a placeholder.
    fn plain(text: String) -> Self {
        let mut hl = Highlighter::plain();
        Self {
            lines: text.lines().map(|l| hl.line(l)).collect(),
            text,
            is_file: false,
            markdown: false,
        }
    }
}

/// The preview of the file at `root/path`: read (capped, binary-guarded)
/// and highlighted. Never fails — an unreadable file previews as the
/// reason. `None` only when `cancel` fired between the read and the
/// highlight: the cursor has moved on and nobody is waiting.
pub(crate) fn file_preview(
    root: &std::path::Path,
    path: &str,
    cancel: Option<&crate::view_jobs::Cancel>,
) -> Option<Preview> {
    let text = match read_preview(&root.join(path)) {
        Ok(text) => text,
        Err(message) => return Some(Preview::plain(message)),
    };
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return None;
    }
    let mut hl = Highlighter::for_path(path);
    Some(Preview {
        lines: text.lines().map(|l| hl.line(l)).collect(),
        text,
        is_file: true,
        markdown: markdown::is_markdown_path(path),
    })
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
    /// Editor command Enter launches (NEBULA_EDITOR, then the `editor`
    /// setting, default vim), captured at open time.
    pub editor: String,
    pub nodes: Vec<TreeNode>,
    /// Top-level node indices (children of the implicit root).
    pub top: Vec<usize>,
    /// Per-node expansion, honored only while the filter is empty — a live
    /// filter force-expands every hierarchy it keeps.
    pub expanded: Vec<bool>,
    /// Type-to-filter query over file paths; always live.
    pub filter: TextInput,
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
    /// Whether `preview` is real file contents — the case that earns a
    /// line-number gutter. False for directory listings and placeholders.
    pub preview_is_file: bool,
    /// The selected file is markdown: Ctrl+r chooses between the rendered
    /// page and the source.
    pub markdown: bool,
    /// Show markdown as the rendered page rather than the source.
    pub pretty: bool,
    /// The page flowed for the last drawn width, written back during draw
    /// (see [`Rendered`]).
    pub rendered: Option<Rendered>,
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
    /// column` at mouse-down, so the border tracks the pointer instead of
    /// jumping to it.
    pub files_drag: Option<i32>,
    /// BACKGROUND READS: with it, a file's preview is read and highlighted
    /// off the loop and lands in [`TreeBrowser::land_preview`]; without (a
    /// browser built by a test), inline.
    pub jobs: Option<crate::view_jobs::Jobs>,
    /// The `git ls-files` this browser opened ahead of, by ticket: the
    /// tree is empty and says so until [`TreeBrowser::set_files`].
    pub listing: Option<u64>,
    /// The preview in flight, by ticket. The pane keeps the last node's
    /// preview meanwhile (`view_jobs::STALE_GRACE`).
    pub waiting: Option<u64>,
    /// Stops the preview in flight when the cursor moves on.
    pub cancel: crate::view_jobs::Cancel,
}

impl TreeBrowser {
    /// A browser up before its listing is: `b` opens this at once and
    /// `set_files` fills it when `git ls-files` answers. What is typed
    /// into the filter meanwhile is kept and applied to the tree it gets.
    pub fn opening(
        root: PathBuf,
        branch: String,
        editor: String,
        jobs: crate::view_jobs::Jobs,
        listing: u64,
    ) -> Self {
        let mut browser = Self::new(root, branch, editor, Vec::new());
        browser.jobs = Some(jobs);
        browser.listing = Some(listing);
        browser
    }

    /// The listing landed: build the tree, narrow it by whatever the filter
    /// holds by now, and preview where the cursor ends up.
    pub fn set_files(&mut self, files: Vec<String>) {
        let (nodes, top, file_count) = build_nodes(&files);
        self.expanded = vec![false; nodes.len()];
        self.nodes = nodes;
        self.top = top;
        self.file_count = file_count;
        self.listing = None;
        self.rebuild_rows();
        self.selected = self
            .best_row
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.load_preview();
    }

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
            filter: TextInput::new(),
            file_count,
            match_count: file_count,
            rows: Vec::new(),
            best_row: None,
            selected: 0,
            preview: String::new(),
            preview_lines: Vec::new(),
            preview_line_count: 0,
            preview_is_file: false,
            markdown: false,
            pretty: true,
            rendered: None,
            scroll: 0,
            view_height: 0,
            list_area: Rect::default(),
            preview_area: Rect::default(),
            area: Rect::default(),
            files_width: DEFAULT_DIFF_FILES_W,
            files_drag: None,
            jobs: None,
            listing: None,
            waiting: None,
            cancel: crate::view_jobs::Cancel::default(),
        };
        browser.rebuild_rows();
        browser.load_preview();
        browser
    }

    pub fn max_scroll(&self) -> u16 {
        max_scroll(self.preview_line_count, self.view_height)
    }

    /// Clamped relative preview scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = scrolled_by(self.scroll, delta, self.max_scroll());
    }

    /// Screen x of the tree/preview boundary — the column where the preview
    /// panel starts.
    pub fn splitter_x(&self) -> u16 {
        self.area.x + self.files_width
    }

    /// Move the tree/preview boundary to `boundary_x`, clamped so the tree
    /// keeps `MIN_DIFF_FILES_W` and the preview keeps `MIN_DIFF_PANE_W`.
    pub fn set_files_width(&mut self, boundary_x: i32) {
        if let Some(width) = clamp_files_width(self.area, boundary_x) {
            self.files_width = width;
        }
    }

    /// First visible row of the tree's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    /// Clamped absolute selection; reloads the preview when it moved.
    pub fn select(&mut self, index: i64) {
        let clamped = clamp_selection(index, self.rows.len());
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
        let visible = visible_rows(&self.nodes, &self.top, &self.expanded, &self.filter);
        self.rows = visible.rows;
        self.best_row = visible.best_row;
        self.match_count = visible.match_count.unwrap_or(self.file_count);
    }

    /// Reload the preview for the current selection and reset the scroll.
    /// Never fails: errors become the displayed text (the `diff_for` rule).
    /// Real file contents get syntax-highlighted; directory listings and
    /// placeholder messages stay plain. A directory's listing is built on
    /// the spot; a file's is read off the loop when there are BACKGROUND
    /// READS to read it with, the pane holding what it showed until the
    /// read lands ([`TreeBrowser::land_preview`]) or is slow
    /// ([`TreeBrowser::preview_slow`]).
    pub fn load_preview(&mut self) {
        // Whatever was being read is no longer under the cursor.
        self.cancel.cancel();
        self.waiting = None;
        let preview = match self.selected_node() {
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
                Preview::plain(listing)
            }
            Some(n) => match self.jobs.clone() {
                Some(jobs) => {
                    let path = n.path.clone();
                    let ticket = crate::view_jobs::ticket();
                    self.waiting = Some(ticket);
                    self.cancel = crate::view_jobs::Cancel::default();
                    let (root, cancel) = (self.root.clone(), self.cancel.clone());
                    jobs.run_with_grace(ticket, move || {
                        let preview = file_preview(&root, &path, Some(&cancel))?;
                        Some(crate::view_jobs::Answer::Preview {
                            ticket,
                            preview: Box::new(preview),
                        })
                    });
                    return;
                }
                None => file_preview(&self.root, &n.path, None).unwrap_or_default(),
            },
            None => Preview::default(),
        };
        self.set_preview(preview);
    }

    fn set_preview(&mut self, preview: Preview) {
        self.scroll = 0;
        self.preview_is_file = preview.is_file;
        self.markdown = preview.markdown;
        self.rendered = None;
        self.preview_line_count = preview.lines.len();
        self.preview_lines = preview.lines;
        self.preview = preview.text;
    }

    /// A background preview came back: shown when it is the one the cursor
    /// is waiting on, dropped when the cursor has moved on since.
    pub fn land_preview(&mut self, ticket: u64, preview: Preview) {
        if self.waiting == Some(ticket) {
            self.waiting = None;
            self.set_preview(preview);
        }
    }

    /// The preview in flight has outlasted the grace the last node's
    /// preview was kept for: say so rather than leave one file's text
    /// under another's name. Still waiting — the read lands over this.
    pub fn preview_slow(&mut self, ticket: u64) {
        if self.waiting == Some(ticket) {
            self.set_preview(Preview::plain("loading…".to_string()));
        }
    }

    /// The preview is the rendered markdown page rather than the source.
    pub fn renders_markdown(&self) -> bool {
        self.markdown && self.pretty
    }

    /// Ctrl+r: the other view of a markdown file (the FILE TABS' `m`).
    pub fn toggle_pretty(&mut self) {
        self.pretty = !self.pretty;
        self.rendered = None;
        if !self.pretty {
            self.preview_line_count = self.preview_lines.len();
        }
    }
}

/// How much of a file's head the binary test reads: git's own 8 KiB.
const BINARY_SNIFF_BYTES: usize = 8192;

/// git's own test for "not text": a NUL byte anywhere in the first 8 KiB.
/// The preview pane says `(binary file)` on it; `nebula open` refuses the
/// file outright on it, since a tab of a PNG's bytes shows nobody anything.
pub(crate) fn looks_binary(head: &[u8]) -> bool {
    head.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0)
}

/// Is this a text file by that test? Reads only the head. An unreadable
/// file is the caller's error to word.
pub(crate) fn is_text_file(path: &std::path::Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = Vec::with_capacity(BINARY_SNIFF_BYTES);
    std::fs::File::open(path)?
        .take(BINARY_SNIFF_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(!looks_binary(&head))
}

/// File contents for a preview pane (this browser's, or the FILE TABS'),
/// capped and binary-guarded. `Err` is the placeholder/error message to
/// display (unhighlighted).
pub(crate) fn read_preview(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Err(format!("couldn't read file: {e}")),
    };
    let mut bytes = Vec::new();
    if let Err(e) = file
        .take((MAX_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        return Err(format!("couldn't read file: {e}"));
    }
    if looks_binary(&bytes) {
        return Err("(binary file)".to_string());
    }
    let byte_capped = bytes.len() > MAX_PREVIEW_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PREVIEW_BYTES)]);
    if text.trim().is_empty() {
        return Err("(empty file)".to_string());
    }
    let out = cap_lines(&text, MAX_PREVIEW_LINES, byte_capped);
    // ratatui doesn't expand tabs; keep columns readable.
    Ok(out.replace('\t', "    "))
}

/// What a tree shows for one filter: see [`visible_rows`].
pub(crate) struct VisibleRows {
    pub rows: Vec<TreeRow>,
    /// Row of the best-scoring file; `None` when the filter is empty.
    pub best_row: Option<usize>,
    /// Files matching the filter; `None` when it is empty and every file
    /// does (the caller knows its own file count).
    pub match_count: Option<usize>,
}

/// The visible rows of a node arena, in tree order. An empty filter walks
/// the expansion state; otherwise every file whose path fuzzy-matches is
/// kept along with its ancestor directories, all forced open. Shared with
/// the DIFF VIEWER's tree list (`diff_tree`), which folds the same way.
pub(crate) fn visible_rows(
    nodes: &[TreeNode],
    top: &[usize],
    expanded: &[bool],
    filter: &str,
) -> VisibleRows {
    let mut rows = Vec::new();
    if filter.is_empty() {
        let mut stack: Vec<usize> = top.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            rows.push(TreeRow {
                node: i,
                positions: Vec::new(),
            });
            if nodes[i].is_dir && expanded[i] {
                for &c in nodes[i].children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        return VisibleRows {
            rows,
            best_row: None,
            match_count: None,
        };
    }
    // Match files on their full relative path (the file-finder rule, so
    // "tui/app" works), then include every ancestor directory.
    let mut include = vec![false; nodes.len()];
    let mut name_positions: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut scores: Vec<Option<i32>> = vec![None; nodes.len()];
    let mut match_count = 0;
    let mut matcher = crate::fuzzy::Matcher::new(filter);
    for i in 0..nodes.len() {
        if nodes[i].is_dir {
            continue;
        }
        let Some(m) = matcher.matches(&nodes[i].path) else {
            continue;
        };
        match_count += 1;
        // Map full-path match positions onto the displayed name; a hit
        // landing in the directory prefix lights nothing on this row.
        let name_chars = nodes[i].name.chars().count();
        let offset = nodes[i].path.chars().count() - name_chars;
        name_positions[i] = m
            .positions
            .iter()
            .filter(|&&p| p >= offset)
            .map(|p| p - offset)
            .collect();
        scores[i] = Some(m.score);
        include[i] = true;
        let mut parent = nodes[i].parent;
        while let Some(p) = parent {
            if include[p] {
                break;
            }
            include[p] = true;
            parent = nodes[p].parent;
        }
    }
    let mut best: Option<(i32, usize)> = None;
    let mut stack: Vec<usize> = top.iter().rev().copied().collect();
    while let Some(i) = stack.pop() {
        if !include[i] {
            continue;
        }
        let row = rows.len();
        rows.push(TreeRow {
            node: i,
            positions: std::mem::take(&mut name_positions[i]),
        });
        if let Some(score) = scores[i] {
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, row));
            }
        }
        if nodes[i].is_dir {
            for &c in nodes[i].children.iter().rev() {
                stack.push(c);
            }
        }
    }
    VisibleRows {
        rows,
        best_row: best.map(|(_, row)| row),
        match_count: Some(match_count),
    }
}

/// Build the node arena from the git listing: directories are implied by
/// the paths, children sorted directories-first then by name.
pub(crate) fn build_nodes(files: &[String]) -> (Vec<TreeNode>, Vec<usize>, usize) {
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
        assert!(
            b.preview.starts_with("couldn't read file:"),
            "{}",
            b.preview
        );
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

    /// The test `nebula open` refuses a file on, shared with the preview:
    /// git's NUL in the first 8 KiB. A PNG's header has one; text, however
    /// odd its characters, has none; an empty file is not binary.
    #[test]
    fn is_text_file_is_gits_nul_test() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        let text = dir.path().join("notes.md");
        std::fs::write(&text, "# notes\nwith \u{e9} and \u{2192} in it\n").unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, "").unwrap();
        assert!(!is_text_file(&png).unwrap());
        assert!(is_text_file(&text).unwrap());
        assert!(is_text_file(&empty).unwrap());
        assert!(is_text_file(&dir.path().join("missing")).is_err());
        assert!(!looks_binary(b"plain"));
        assert!(looks_binary(b"a\0b"));
    }
}
