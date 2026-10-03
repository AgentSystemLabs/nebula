//! The GRAPH: the lower section of the DIFF VIEWER's SOURCE CONTROL
//! sidebar (`g`), under the CHANGES. The checkout's commits (HEAD, every
//! local branch and every remote-tracking one, newest first and drawn the
//! way `git log --graph` draws them) with each commit's refs, author and
//! age, and an arrow on the ones HEAD's upstream does not share yet: `↑`
//! for a commit a push would send, `↓` for one a pull would bring.
//!
//! The cursor's commit is read in the right pane (`git show --stat`).
//! `Enter`, `→` or a click unfolds a commit into its files right under it,
//! each diffed against the commit's first parent when the cursor is on it.
//! Reading only: nothing in this module writes to the repository.

use crate::app::{DiffView, Place};
use crate::git_diff::{cap_lines, run_git, DiffFile, MAX_DIFF_LINES};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The most commits one read lists, newest first: a branch's recent story,
/// read in well under a second on a large repository. The whole history
/// is `git log`'s job.
const MAX_COMMITS: &str = "500";

/// Between the fields of a log line; git puts none in a ref or a subject.
const SEP: char = '\u{1f}';

/// The cache and `DiffView::shown` key of the checkout's `git status`, what
/// the right pane reads on a section header: no commit hash or path can
/// look like it.
pub const WORKING: &str = ":working";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// HEAD, with the branch it is on (an empty name when detached).
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    pub name: String,
}

impl Ref {
    pub fn label(&self) -> String {
        match (self.kind, self.name.as_str()) {
            (RefKind::Head, "") => "HEAD".into(),
            (RefKind::Head, branch) => format!("HEAD → {branch}"),
            (RefKind::Tag, tag) => format!("tag: {tag}"),
            (_, name) => name.into(),
        }
    }
}

/// Where a commit sits against HEAD's upstream branch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Side {
    /// On both, or there is no upstream to compare with.
    #[default]
    Shared,
    /// On HEAD only: a push would send it.
    Ahead,
    /// On the upstream only: a pull would bring it.
    Behind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub refs: Vec<Ref>,
    pub author: String,
    /// Committer date, unix seconds.
    pub time: i64,
    pub subject: String,
    pub side: Side,
}

impl Commit {
    /// `(HEAD → main, origin/main)`, empty for a commit no ref points at.
    pub fn refs_label(&self) -> String {
        if self.refs.is_empty() {
            return String::new();
        }
        let labels: Vec<String> = self.refs.iter().map(Ref::label).collect();
        format!("({})", labels.join(", "))
    }

    /// What the filter matches and a filtered row shows: the short hash,
    /// the refs and the subject.
    pub fn haystack(&self) -> String {
        let refs = self.refs_label();
        if refs.is_empty() {
            format!("{} {}", self.short, self.subject)
        } else {
            format!("{} {refs} {}", self.short, self.subject)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Commit(usize),
    /// One of an unfolded commit's files: (commit, file).
    File(usize, usize),
    /// Under an unfolded commit with no files to list: still being read,
    /// none changed, or git's refusal.
    Note(usize),
    /// A line of graph between two commits.
    Graph,
}

impl Entry {
    /// Rows the cursor rests on; it steps over the rest.
    pub fn selectable(self) -> bool {
        matches!(self, Entry::Commit(_) | Entry::File(..))
    }
}

#[derive(Debug, Clone)]
pub struct LogRow {
    pub entry: Entry,
    /// The `git log --graph` line this row draws (an index into
    /// `GitLog::lines`; a file row repeats its commit's); none while the
    /// filter narrows the list.
    pub line: Option<usize>,
    /// The filter's matched char positions in the commit's `haystack`.
    pub positions: Vec<usize>,
}

/// An unfolded commit's files: `None` while they are read.
pub type Unfolded = Option<Result<Vec<DiffFile>, String>>;

/// One read of a checkout's history, off the loop.
#[derive(Debug, Default)]
pub struct LogRead {
    pub commits: Vec<Commit>,
    pub lines: Vec<(String, Option<usize>)>,
}

/// The GRAPH section. Shared behind `Arc`s: the views are cloned every
/// frame.
#[derive(Debug, Clone, Default)]
pub struct GitLog {
    pub commits: Arc<[Commit]>,
    /// Every line `git log --graph` drew: the graph, and the commit on the
    /// line (none on a line that only connects two).
    pub lines: Arc<[(String, Option<usize>)]>,
    /// The rows on screen: every graph line, or the commits the filter
    /// matched, best first; each unfolded commit followed by its files.
    pub rows: Vec<LogRow>,
    pub selected: usize,
    /// The filter `rows` were built for (the DIFF VIEWER's own).
    query: String,
    /// Commits unfolded into their files, by hash.
    pub unfolded: HashMap<String, Unfolded>,
    /// The read in flight, by ticket.
    pub reading: Option<u64>,
    /// Why the last read failed.
    pub error: Option<String>,
}

impl GitLog {
    pub fn selected_entry(&self) -> Option<Entry> {
        self.rows.get(self.selected).map(|r| r.entry)
    }

    /// The cursor's commit, or the commit of the file it is on.
    pub fn selected_commit(&self) -> Option<&Commit> {
        match self.selected_entry()? {
            Entry::Commit(c) | Entry::File(c, _) => self.commits.get(c),
            _ => None,
        }
    }

    pub fn file_at(&self, commit: usize, file: usize) -> Option<&DiffFile> {
        let sha = &self.commits.get(commit)?.sha;
        match self.unfolded.get(sha)? {
            Some(Ok(files)) => files.get(file),
            _ => None,
        }
    }

    /// What a row reads in the right pane, as a cache key: a commit's hash,
    /// or `hash:path` for one of its files.
    fn key_of(&self, entry: Entry) -> Option<String> {
        match entry {
            Entry::Commit(c) => Some(self.commits.get(c)?.sha.clone()),
            Entry::File(c, f) => Some(format!(
                "{}:{}",
                self.commits.get(c)?.sha,
                self.file_at(c, f)?.path
            )),
            _ => None,
        }
    }

    pub fn selected_key(&self) -> Option<String> {
        self.key_of(self.selected_entry()?)
    }

    /// What an unfolded commit with no file rows says instead.
    pub fn note(&self, commit: usize) -> String {
        let unfolded = self
            .commits
            .get(commit)
            .and_then(|c| self.unfolded.get(&c.sha));
        match unfolded {
            Some(Some(Err(msg))) => msg.clone(),
            Some(Some(Ok(_))) => "no file changes".into(),
            _ => "loading…".into(),
        }
    }

    /// How many commits HEAD's upstream lacks, and how many HEAD lacks.
    pub fn sides(&self) -> (usize, usize) {
        let count = |side| self.commits.iter().filter(|c| c.side == side).count();
        (count(Side::Ahead), count(Side::Behind))
    }

    /// The next commit down from the cursor: what is read ahead.
    pub fn commit_after_cursor(&self) -> Option<&Commit> {
        self.rows
            .iter()
            .skip(self.selected + 1)
            .find_map(|r| match r.entry {
                Entry::Commit(c) => self.commits.get(c),
                _ => None,
            })
    }

    /// Put the cursor on row `target` (clamped), or on the nearest row it
    /// can rest on in the way it is moving. True when it moved.
    pub fn select_toward(&mut self, target: usize, down: bool) -> bool {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return false;
        };
        let target = target.min(last);
        let ok = |i: &usize| self.rows[*i].entry.selectable();
        let below = (target..self.rows.len()).find(ok);
        let above = (0..=target).rev().find(ok);
        let found = if down {
            below.or(above)
        } else {
            above.or(below)
        };
        let Some(row) = found else {
            return false;
        };
        let moved = row != self.selected;
        self.selected = row;
        moved
    }

    /// [`GitLog::select_toward`] row `index`, moving the way it lies.
    pub fn select(&mut self, index: i64) -> bool {
        let down = index >= self.selected as i64;
        self.select_toward(index.max(0) as usize, down)
    }

    fn select_key(&mut self, key: &str) -> bool {
        let row =
            (0..self.rows.len()).find(|&i| self.key_of(self.rows[i].entry).as_deref() == Some(key));
        if let Some(row) = row {
            self.selected = row;
        }
        row.is_some()
    }

    pub fn select_sha(&mut self, sha: &str) -> bool {
        self.select_key(sha)
    }

    /// Where the cursor starts: HEAD's commit.
    pub fn go_home(&mut self) {
        let head = self.rows.iter().position(|r| match r.entry {
            Entry::Commit(c) => self.commits[c]
                .refs
                .iter()
                .any(|rf| rf.kind == RefKind::Head),
            _ => false,
        });
        self.selected = 0;
        if let Some(row) = head {
            self.selected = row;
        } else {
            self.select_toward(0, true);
        }
    }

    /// Rebuild the rows for the filter `query`. An empty one lists every
    /// graph line and keeps the cursor's row if it is still there (or goes
    /// home); one with text lands on the best match. True when the cursor
    /// reads something else than it did.
    pub fn apply_filter(&mut self, query: &str) -> bool {
        let before = self.selected_key();
        self.query = query.to_string();
        self.rebuild();
        let kept = query.trim().is_empty() && before.as_deref().is_some_and(|k| self.select_key(k));
        if !kept {
            if query.trim().is_empty() {
                self.go_home();
            } else {
                self.selected = 0;
                self.select_toward(0, true);
            }
        }
        self.selected_key() != before
    }

    /// Rebuild the rows as they stand (a commit folded, its files read),
    /// the cursor kept on what it reads, or near where it was.
    fn refresh(&mut self) {
        let before = self.selected_key();
        let row = self.selected;
        self.rebuild();
        if !before.as_deref().is_some_and(|k| self.select_key(k)) {
            self.select_toward(row, false);
        }
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        if self.query.trim().is_empty() {
            for (n, (_, commit)) in self.lines.iter().enumerate() {
                match commit {
                    Some(c) => self.push_commit(&mut rows, *c, Some(n), Vec::new()),
                    None => rows.push(LogRow {
                        entry: Entry::Graph,
                        line: Some(n),
                        positions: Vec::new(),
                    }),
                }
            }
        } else {
            let hay: Vec<String> = self.commits.iter().map(Commit::haystack).collect();
            for (c, positions) in crate::fuzzy::rank(&self.query, hay.iter().map(String::as_str)) {
                self.push_commit(&mut rows, c, None, positions);
            }
        }
        self.rows = rows;
    }

    fn push_commit(
        &self,
        rows: &mut Vec<LogRow>,
        c: usize,
        line: Option<usize>,
        positions: Vec<usize>,
    ) {
        rows.push(LogRow {
            entry: Entry::Commit(c),
            line,
            positions,
        });
        let under = |entry| LogRow {
            entry,
            line,
            positions: Vec::new(),
        };
        match self.unfolded.get(&self.commits[c].sha) {
            None => {}
            Some(Some(Ok(files))) if !files.is_empty() => {
                rows.extend((0..files.len()).map(|f| under(Entry::File(c, f))));
            }
            Some(_) => rows.push(under(Entry::Note(c))),
        }
    }

    /// A read landed: the list is replaced and the cursor stays on what it
    /// read when the fresh list still has it.
    fn land(&mut self, read: LogRead) {
        let before = self.selected_key();
        self.commits = read.commits.into();
        self.lines = read.lines.into();
        self.error = None;
        self.rebuild();
        if !before.as_deref().is_some_and(|k| self.select_key(k)) {
            self.go_home();
        }
    }
}

/// `git log --graph` over HEAD, every local branch and every remote one.
pub fn read_log(root: &Path) -> Result<LogRead, String> {
    if !crate::git_diff::has_head(root) {
        return Ok(LogRead::default());
    }
    let output = run_git(
        root,
        &[
            "log",
            "--graph",
            "--date-order",
            "--decorate=full",
            "--no-color",
            "--format=%x1f%H%x1f%h%x1f%D%x1f%an%x1f%ct%x1f%s",
            "-n",
            MAX_COMMITS,
            "HEAD",
            "--branches",
            "--remotes",
            "--",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git log failed: {}", stderr.trim()));
    }
    let (mut commits, lines) = parse_log(&String::from_utf8_lossy(&output.stdout));
    mark_sides(root, &mut commits);
    Ok(LogRead { commits, lines })
}

/// Split `git log --graph` output into its commits and its lines: a line
/// carrying the field separator is a commit's (the graph left of it), one
/// without only connects two.
pub fn parse_log(text: &str) -> (Vec<Commit>, Vec<(String, Option<usize>)>) {
    let mut commits = Vec::new();
    let mut lines = Vec::new();
    for line in text.lines() {
        let Some((graph, rest)) = line.split_once(SEP) else {
            lines.push((line.trim_end().to_string(), None));
            continue;
        };
        let f: Vec<&str> = rest.splitn(6, SEP).collect();
        let [sha, short, refs, author, time, subject] = f[..] else {
            continue;
        };
        lines.push((graph.trim_end().to_string(), Some(commits.len())));
        commits.push(Commit {
            sha: sha.into(),
            short: short.into(),
            refs: parse_refs(refs),
            author: author.into(),
            time: time.parse().unwrap_or(0),
            subject: subject.into(),
            side: Side::Shared,
        });
    }
    (commits, lines)
}

/// `%D` under `--decorate=full`: `HEAD -> refs/heads/main, refs/remotes/
/// origin/main, tag: refs/tags/v1`. A remote's `HEAD` says nothing its
/// branch does not, and refs outside branches and tags (a stash) are
/// left out.
pub fn parse_refs(decorations: &str) -> Vec<Ref> {
    decorations
        .split(", ")
        .filter_map(|d| {
            let (kind, name) = if let Some(branch) = d.strip_prefix("HEAD -> ") {
                (
                    RefKind::Head,
                    branch.strip_prefix("refs/heads/").unwrap_or(branch),
                )
            } else if d == "HEAD" {
                (RefKind::Head, "")
            } else if let Some(tag) = d.strip_prefix("tag: ") {
                (RefKind::Tag, tag.strip_prefix("refs/tags/").unwrap_or(tag))
            } else if let Some(branch) = d.strip_prefix("refs/heads/") {
                (RefKind::Local, branch)
            } else {
                let remote = d
                    .strip_prefix("refs/remotes/")
                    .filter(|r| !r.ends_with("/HEAD"))?;
                (RefKind::Remote, remote)
            };
            Some(Ref {
                kind,
                name: name.into(),
            })
        })
        .collect()
}

/// Arrow the commits HEAD and its upstream do not share. No upstream (or a
/// detached HEAD) marks nothing.
fn mark_sides(root: &Path, commits: &mut [Commit]) {
    let Ok(output) = run_git(root, &["rev-list", "--left-right", "HEAD...@{upstream}"]) else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let sides: HashMap<&str, Side> = text
        .lines()
        .filter_map(|l| match l.split_at_checked(1)? {
            ("<", sha) => Some((sha, Side::Ahead)),
            (">", sha) => Some((sha, Side::Behind)),
            _ => None,
        })
        .collect();
    for commit in commits {
        if let Some(side) = sides.get(commit.sha.as_str()) {
            commit.side = *side;
        }
    }
}

/// The files a commit changed against its first parent (a root commit:
/// everything it added).
pub fn read_commit_files(root: &Path, sha: &str) -> Result<Vec<DiffFile>, String> {
    let output = run_git(
        root,
        &[
            "show",
            "--format=",
            "--name-status",
            "-z",
            "-M",
            "--diff-merges=first-parent",
            sha,
            "--",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git show failed: {}", stderr.trim()));
    }
    Ok(parse_name_status_z(&output.stdout))
}

/// `--name-status -z`: `M\0path\0`, and for a rename or copy
/// `R100\0old\0new\0`.
pub fn parse_name_status_z(bytes: &[u8]) -> Vec<DiffFile> {
    let mut fields = bytes
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let Some(x) = status.chars().next() else {
            continue;
        };
        let Some(first) = fields.next() else {
            break;
        };
        let (path, orig_path) = if matches!(x, 'R' | 'C') {
            match fields.next() {
                Some(new) => (new, Some(first)),
                None => break,
            }
        } else {
            (first, None)
        };
        files.push(DiffFile {
            path,
            orig_path,
            xy: [x, ' '],
        });
    }
    files
}

/// The right pane for a commit: its header, message and stat; for
/// [`WORKING`], the checkout's `git status` with where it stands against
/// its upstream. Never fails: an error is the text shown.
pub fn summary_for(root: &Path, key: &str) -> String {
    let output = if key == WORKING {
        run_git(root, &["status", "--short", "--branch"])
    } else {
        run_git(
            root,
            &[
                "show",
                "--stat",
                "--decorate=short",
                "--format=fuller",
                "--no-color",
                "--diff-merges=first-parent",
                key,
                "--",
            ],
        )
    };
    match output {
        Ok(o) if o.status.success() => {
            cap_lines(&String::from_utf8_lossy(&o.stdout), MAX_DIFF_LINES, false)
        }
        Ok(o) => format!("git failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => e,
    }
}

/// Read the GRAPH (again: a commit or a fetch may have landed since), the
/// last read showing until it lands.
pub fn request_log(view: &mut DiffView) {
    let ticket = crate::view_jobs::ticket();
    let log = view.log.get_or_insert_with(GitLog::default);
    log.reading = Some(ticket);
    let root = view.root.clone();
    match view.jobs.clone() {
        Some(jobs) => jobs.run(move || {
            Some(crate::view_jobs::Answer::Log {
                ticket,
                result: read_log(&root),
            })
        }),
        None => land_log(view, ticket, read_log(&root)),
    }
}

/// The history read came back.
pub fn land_log(view: &mut DiffView, ticket: u64, result: Result<LogRead, String>) {
    let Some(log) = &mut view.log else {
        return;
    };
    if log.reading != Some(ticket) {
        return;
    }
    log.reading = None;
    match result {
        Ok(read) => {
            log.land(read);
            let query = view.filter.to_string();
            if !query.trim().is_empty() {
                if let Some(log) = &mut view.log {
                    log.apply_filter(&query);
                }
            }
        }
        Err(msg) => log.error = Some(msg),
    }
    if view.settle() || view.place == Place::Graph {
        crate::git_diff::load_selected_diff(view);
    }
}

/// Unfold the cursor's commit into its files, reading them, or fold it
/// (`open` None flips it). `→` on an unfolded commit steps onto its first
/// file and `←` on a file steps back up onto its commit. True when the
/// cursor's row or the rows changed.
pub fn fold(view: &mut DiffView, open: Option<bool>) -> bool {
    let Some(log) = &mut view.log else {
        return false;
    };
    let (c, on_file) = match log.selected_entry() {
        Some(Entry::Commit(c)) => (c, false),
        Some(Entry::File(c, _)) => (c, true),
        _ => return false,
    };
    let sha = log.commits[c].sha.clone();
    let is_open = log.unfolded.contains_key(&sha);
    if on_file {
        return open == Some(false) && log.select_sha(&sha);
    }
    match (open, is_open) {
        (Some(true), true) => {
            let first = log.rows.get(log.selected + 1).map(|r| r.entry);
            matches!(first, Some(Entry::File(..))) && log.select(log.selected as i64 + 1)
        }
        (Some(false), false) => false,
        (_, true) => {
            log.unfolded.remove(&sha);
            log.refresh();
            true
        }
        (_, false) => {
            log.unfolded.insert(sha.clone(), None);
            log.refresh();
            request_files(view, sha);
            true
        }
    }
}

fn request_files(view: &mut DiffView, sha: String) {
    let (root, id) = (view.root.clone(), view.id);
    match view.jobs.clone() {
        Some(jobs) => jobs.run(move || {
            Some(crate::view_jobs::Answer::CommitFiles {
                view: id,
                result: read_commit_files(&root, &sha),
                sha,
            })
        }),
        None => {
            let result = read_commit_files(&root, &sha);
            land_files(view, id, &sha, result);
        }
    }
}

/// An unfolded commit's files came back: they go in under it, if it is
/// still unfolded.
pub fn land_files(view: &mut DiffView, id: u64, sha: &str, result: Result<Vec<DiffFile>, String>) {
    if id != view.id {
        return;
    }
    let Some(log) = &mut view.log else {
        return;
    };
    match log.unfolded.get_mut(sha) {
        Some(slot) if slot.is_none() => *slot = Some(result),
        _ => return,
    }
    log.refresh();
}

/// What the right pane reads off the loop for a row outside the CHANGES.
enum Read {
    Summary(String),
    File(String, DiffFile),
}

impl Read {
    fn key(&self) -> String {
        match self {
            Read::Summary(key) => key.clone(),
            Read::File(sha, file) => format!("{sha}:{}", file.path),
        }
    }

    fn run(&self, root: &Path) -> String {
        match self {
            Read::Summary(key) => summary_for(root, key),
            Read::File(sha, file) => crate::git_diff::diff_for(root, file, true, Some(sha)),
        }
    }
}

/// Read the right pane for the cursor when it is not on a changed file: a
/// section header's `git status`, a commit, or one of its files. A commit
/// never changes, so one read of it is the last; the status is read again
/// each time, as a changed file's diff is.
pub fn load_selected(view: &mut DiffView) {
    view.waiting = None;
    let read = match view.place {
        Place::Graph => {
            let Some(log) = &view.log else {
                return;
            };
            match log.selected_entry() {
                Some(Entry::Commit(c)) => Read::Summary(log.commits[c].sha.clone()),
                Some(Entry::File(c, f)) => match log.file_at(c, f) {
                    Some(file) => Read::File(log.commits[c].sha.clone(), file.clone()),
                    None => return,
                },
                _ => {
                    let text = match (&log.error, log.reading) {
                        (Some(msg), _) => msg.clone(),
                        (None, None) if log.commits.is_empty() => "no commits yet".into(),
                        _ => String::new(),
                    };
                    view.show_diff(None, text, false);
                    return;
                }
            }
        }
        _ => Read::Summary(WORKING.into()),
    };
    let key = read.key();
    let Some(jobs) = view.jobs.clone() else {
        let text = read.run(&view.root);
        view.show_diff(Some(&key), text, false);
        return;
    };
    if let Some(text) = view.cached(&key) {
        view.show_diff(Some(&key), text.to_string(), false);
        if key != WORKING {
            return;
        }
    }
    let ticket = crate::view_jobs::ticket();
    view.waiting = Some(ticket);
    request(view, &jobs, read, ticket, false);
}

/// The commit after the cursor, read ahead once the cursor's has landed.
pub fn read_ahead(view: &DiffView) {
    if view.place != Place::Graph {
        return;
    }
    let next = view
        .log
        .as_ref()
        .and_then(GitLog::commit_after_cursor)
        .map(|c| c.sha.clone())
        .filter(|sha| view.cached(sha).is_none());
    if let (Some(sha), Some(jobs)) = (next, view.jobs.clone()) {
        request(
            view,
            &jobs,
            Read::Summary(sha),
            crate::view_jobs::ticket(),
            true,
        );
    }
}

fn request(
    view: &DiffView,
    jobs: &crate::view_jobs::Jobs,
    read: Read,
    ticket: u64,
    prefetch: bool,
) {
    let (root, id) = (view.root.clone(), view.id);
    let work = move || {
        Some(crate::view_jobs::Answer::DiffText {
            view: id,
            ticket,
            diff: read.run(&root),
            path: read.key(),
            prefetch,
        })
    };
    if prefetch {
        jobs.run(work);
    } else {
        jobs.run_with_grace(ticket, work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn commit(sha: &str, refs: Vec<Ref>) -> Commit {
        Commit {
            sha: sha.into(),
            short: sha.into(),
            refs,
            author: "a".into(),
            time: 0,
            subject: format!("subject {sha}"),
            side: Side::Shared,
        }
    }

    fn head() -> Vec<Ref> {
        vec![Ref {
            kind: RefKind::Head,
            name: "main".into(),
        }]
    }

    /// A log as `git log --graph` lays out a merge: two commits side by
    /// side, the lines joining them, HEAD on the third.
    fn log() -> GitLog {
        let read = LogRead {
            commits: vec![
                commit("c0", Vec::new()),
                commit("c1", Vec::new()),
                commit("c2", head()),
            ],
            lines: vec![
                ("*".into(), Some(0)),
                ("|\\".into(), None),
                ("| *".into(), Some(1)),
                ("|/".into(), None),
                ("*".into(), Some(2)),
            ],
        };
        let mut log = GitLog::default();
        log.land(read);
        log
    }

    fn modified(path: &str) -> DiffFile {
        DiffFile {
            path: path.into(),
            orig_path: None,
            xy: ['M', ' '],
        }
    }

    #[test]
    fn parse_log_splits_commits_from_the_lines_between_them() {
        let text = "* \x1faaaa\x1faa\x1fHEAD -> refs/heads/main, refs/remotes/origin/main\x1fAnn\x1f100\x1ffix: a\x1fb\n\
                    |\\  \n\
                    | * \x1fbbbb\x1fbb\x1f\x1fBob\x1f90\x1fadd b\n";
        let (commits, lines) = parse_log(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(
            commits[0].subject, "fix: a\x1fb",
            "the subject keeps a stray separator"
        );
        assert_eq!(commits[0].time, 100);
        assert_eq!(commits[0].refs_label(), "(HEAD → main, origin/main)");
        assert!(commits[1].refs.is_empty());
        assert_eq!(
            lines,
            vec![
                ("*".to_string(), Some(0)),
                ("|\\".to_string(), None),
                ("| *".to_string(), Some(1)),
            ]
        );
    }

    #[test]
    fn parse_refs_names_each_kind_and_drops_a_remotes_head() {
        let refs = parse_refs(
            "HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, \
             tag: refs/tags/v1, refs/heads/feat, refs/stash",
        );
        let kinds: Vec<(RefKind, &str)> = refs.iter().map(|r| (r.kind, r.name.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (RefKind::Head, "main"),
                (RefKind::Remote, "origin/main"),
                (RefKind::Tag, "v1"),
                (RefKind::Local, "feat"),
            ]
        );
        assert_eq!(parse_refs("HEAD")[0].label(), "HEAD", "detached");
        assert!(parse_refs("").is_empty());
    }

    #[test]
    fn parse_name_status_z_reads_renames_old_then_new() {
        let files = parse_name_status_z(b"M\0src/a.rs\0R100\0old.rs\0new.rs\0A\0b\0");
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].xy, ['M', ' ']);
        assert_eq!(files[1].path, "new.rs");
        assert_eq!(files[1].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(files[2].path, "b");
        assert!(parse_name_status_z(b"").is_empty());
    }

    /// The cursor starts on HEAD's commit, and walking it steps over the
    /// graph's connecting lines whichever way it goes.
    #[test]
    fn the_cursor_starts_on_head_and_steps_over_graph_lines() {
        let mut log = log();
        assert_eq!(log.selected_key().as_deref(), Some("c2"));
        assert!(log.select(log.selected as i64 - 1));
        assert_eq!(log.selected_key().as_deref(), Some("c1"), "up over |/");
        assert!(log.select(log.selected as i64 - 1));
        assert_eq!(log.selected_key().as_deref(), Some("c0"), "up over |\\");
        assert!(!log.select(-5), "clamped at the top");
        assert!(log.select(1));
        assert_eq!(log.selected_key().as_deref(), Some("c1"), "down over |\\");
        assert_eq!(
            log.commit_after_cursor().map(|c| c.sha.as_str()),
            Some("c2")
        );
    }

    /// Typing narrows the list to matching commits, best first and with no
    /// graph; clearing it puts the graph back with the cursor where it was.
    #[test]
    fn the_filter_narrows_to_commits_and_clearing_keeps_the_cursor() {
        let mut log = log();
        assert!(log.apply_filter("c1"));
        assert_eq!(log.rows.len(), 1);
        assert_eq!(log.selected_key().as_deref(), Some("c1"));
        assert!(log.rows[0].line.is_none());

        assert!(!log.apply_filter(""), "still on c1");
        assert_eq!(log.rows.len(), 5);
        assert_eq!(log.selected_key().as_deref(), Some("c1"));
    }

    /// A fresh read keeps the cursor on its commit.
    #[test]
    fn a_fresh_read_keeps_the_cursor_on_its_commit() {
        let mut log = log();
        log.select_sha("c0");
        let read = LogRead {
            commits: vec![commit("new", head()), commit("c0", Vec::new())],
            lines: vec![("*".into(), Some(0)), ("*".into(), Some(1))],
        };
        log.land(read);
        assert_eq!(log.selected_key().as_deref(), Some("c0"));
    }

    /// An unfolded commit lists its files right under it, each on its own
    /// key; folding it again puts the cursor back on the commit.
    #[test]
    fn an_unfolded_commit_lists_its_files_under_it() {
        let mut log = log();
        log.select_sha("c1");
        log.unfolded.insert("c1".into(), None);
        log.refresh();
        assert_eq!(log.rows[3].entry, Entry::Note(1), "loading…");
        assert_eq!(log.note(1), "loading…");

        *log.unfolded.get_mut("c1").unwrap() = Some(Ok(vec![modified("a.rs"), modified("b.rs")]));
        log.refresh();
        assert_eq!(
            log.selected_key().as_deref(),
            Some("c1"),
            "the cursor kept its row"
        );
        assert!(log.select(log.selected as i64 + 1));
        assert_eq!(log.selected_key().as_deref(), Some("c1:a.rs"));
        assert_eq!(log.selected_commit().map(|c| c.sha.as_str()), Some("c1"));

        log.unfolded.remove("c1");
        log.refresh();
        assert_eq!(log.rows.len(), 5);
        assert!(log.selected_entry().is_some_and(Entry::selectable));
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = run_git(repo, args).unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_file(repo: &Path, name: &str, msg: &str) {
        std::fs::write(repo.join(name), format!("{name}\n")).unwrap();
        git(repo, &["add", name]);
        git(repo, &["commit", "-qm", msg]);
    }

    /// Against real repositories: a clone that has committed one change
    /// and not pulled another lists both, its own arrowed `↑` and the
    /// remote's `↓`, the remote branch named as one; a commit's files and
    /// a file's diff read against its parent.
    #[test]
    fn read_log_lists_local_and_remote_commits_against_the_upstream() {
        let dir = tempfile::tempdir().unwrap();
        let origin: PathBuf = dir.path().join("origin");
        let clone: PathBuf = dir.path().join("clone");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["config", "user.email", "t@t"]);
        git(&origin, &["config", "user.name", "t"]);
        commit_file(&origin, "a", "first");
        let out = std::process::Command::new("git")
            .args(["clone", "-q"])
            .arg(&origin)
            .arg(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());
        git(&clone, &["config", "user.email", "t@t"]);
        git(&clone, &["config", "user.name", "t"]);
        commit_file(&clone, "mine", "local work");
        commit_file(&origin, "theirs", "remote work");
        git(&clone, &["fetch", "-q"]);

        let read = read_log(&clone).unwrap();
        let by_subject = |s: &str| read.commits.iter().find(|c| c.subject == s).unwrap();
        assert_eq!(by_subject("local work").side, Side::Ahead);
        assert_eq!(by_subject("remote work").side, Side::Behind);
        assert_eq!(by_subject("first").side, Side::Shared);
        assert_eq!(by_subject("local work").refs_label(), "(HEAD → main)");
        assert_eq!(by_subject("remote work").refs_label(), "(origin/main)");

        let local = by_subject("local work");
        let files = read_commit_files(&clone, &local.sha).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "mine");
        assert_eq!(files[0].xy, ['A', ' ']);
        let diff = crate::git_diff::diff_for(&clone, &files[0], true, Some(&local.sha));
        assert!(diff.contains("+mine"), "{diff}");
        assert!(summary_for(&clone, &local.sha).contains("local work"));
        assert!(summary_for(&clone, WORKING).contains("ahead 1, behind 1"));
    }

    #[test]
    fn read_log_on_an_unborn_head_lists_no_commits() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let read = read_log(dir.path()).unwrap();
        assert!(read.commits.is_empty());
    }
}
