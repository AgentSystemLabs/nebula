//! The ISSUES MODAL: the selected project's open GitHub issues, listed
//! newest first down the left, the one under the cursor read on the right,
//! and two ways to put an agent on it — a QUICK PROMPT (`Enter`) or one
//! of the saved AGENT PRESETS (`Shift+Tab`), the QUICK PROMPT box's own
//! keys for them. Either launch carries the issue as
//! an [`IssueRef`], and the create that ends it sends the issue URL to the
//! DAEMON (`ClientRequest::CreateAgent::issue_url`), which folds it into the
//! harness's context on every spawn — Claude's appended system prompt, a
//! Codex / Cursor cold spawn's first prompt — so the agent knows which
//! issue the session is for before it reads the first word of the task.
//!
//! `Ctrl+c` (or `Ctrl+y`, the grid's reply key as a chord) leaves a
//! comment on the issue instead: a multi-row box (the task
//! prompts' shape) whose Enter posts the text as you with
//! `gh issue comment`, off the loop, and puts the modal back on its row —
//! the pane says the comment is on its way, and the conversation is read
//! again once it has landed. A post `gh` refused brings the box back with
//! the text, so nothing typed is lost.
//!
//! `Ctrl+e` edits the issue itself, in place: the reading pane becomes a form
//! on its title and description ([`IssueEditor`]), and Enter sends both
//! as one `gh issue edit` off the loop. The form holds until GitHub
//! answers, so a refusal shows `gh`'s reason over text that is still
//! there; a save that took lands on the row at once, and the list is
//! re-asked underneath so the row is GitHub's copy.
//!
//! The list's filter is live from the moment the modal opens, as the
//! DIFF VIEWER's and the FILE FINDER's are: every letter typed narrows
//! the rows to the fuzzy matches of `#15 title` (`fuzzy::rank`), best
//! first, the cursor on the best, and Esc clears it before a second Esc
//! closes. So the verbs are chords — `Ctrl+e`, `Ctrl+c`, `Ctrl+o`,
//! `Ctrl+r` — and the PULL REQUESTS MODAL's filter is this one.
//!
//! Like the pull requests, the issues are the TUI's own business: one
//! `gh issue list` per project — asked in the background once the cursor
//! has rested on the project ([`schedule_prefetch`]), re-asked on a slow
//! beat while it stays selected ([`refresh_selected`]), and again when the
//! modal opens on a list older than [`FRESH`] (or on `r`) — and one
//! `gh issue view` for the comments of the row the cursor rests on, both
//! off the loop with the answer landing on `App::issues_tx`. A `gh` that is
//! missing, unauthenticated, or pointed at a repo with no remote is an
//! ordinary "couldn't ask", said in the pane rather than flashed, and
//! backed off like the open pull requests' list so a machine without `gh`
//! is not asked every beat. Nothing is written to disk: the list is a
//! modal's worth of rows kept in memory, so `i` paints the prefetched rows
//! at once instead of an empty modal while the first answer lands, and a
//! reopen paints the last list while the fresh one lands underneath.
//!
//! The same list is the PROJECT ISSUES GROUP under the Worktrees panel's
//! pull requests (`App::worktree_rows`): one row per open issue, and the
//! pane reads the one the cursor rests on the way it reads a pull request
//! — [`lines`] draws the modal's pane and that one alike — with the
//! comments asked for on the same debounce ([`schedule_detail`]). `p` and
//! `e` on the row are the modal's `Enter` and `Shift+Tab` for it
//! ([`open_prompt_for_row`], [`open_preset_for_row`]).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::{ClientRequest, ProjectId};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{clamp_selection, window_start, App, HitTarget, Overlay};
use crate::markdown::{self, Breaks};
use crate::pr_preview::fit;
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, draw_multiline_input, draw_scroll_marks, empty_list_row,
    fuzzy_highlight_styled, input_spans, panel_block, render_row, row_rect, search_line, truncate,
    visible_positions, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

/// How long a lookup may run before we give up on it — the PR lookups'
/// budget, for the same reason: `gh` retries on a stalled network.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// The most issues one list asks for. `gh` pages past its own 30-row
/// default; a repo with hundreds of open issues would spend several API
/// calls filling rows nobody scrolls to.
pub const LIST_LIMIT: usize = 100;
/// How long the cursor rests on a row before its comments are fetched, so
/// walking the list with `j` fetches only the rows actually paused on.
const DETAIL_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
/// How long the cursor rests on a project before its open issues are asked
/// for in the background — the session prewarm's debounce, for the same
/// reason: walking the project list with j/k must not spawn a `gh` per row
/// passed, only one for the row the cursor settles on.
const PREFETCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
/// The beat a selected project's list is re-asked on once it has proved it
/// has open issues, so the rows `i` paints are never older than this while
/// the project stays selected.
pub(crate) const REFRESH: std::time::Duration = std::time::Duration::from_secs(2 * 60);
/// The backoff after an empty or failed answer: doubling from the floor to
/// the ceiling, so a repo with nothing open (or a machine with no `gh`)
/// settles at the ceiling instead of being asked every beat.
pub(crate) const RECHECK_MIN: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) const RECHECK_MAX: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// How often the open issues of a project the cursor is *not* on are
/// re-asked while PR & ISSUE COUNTS is on (Settings → Experimental) — the
/// open pull requests' sweep beat, for the same budget: one `gh issue list`
/// per project per beat, one project per tick ([`sweep_others`]), twelve
/// calls an hour per project. With the switch off no project but the
/// selected one is ever asked.
pub(crate) const SWEEP_REFRESH: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// A list younger than this is what opening the modal shows, with no second
/// ask: the prefetch that landed as the cursor settled *is* the answer `i`
/// was waiting for. `r` asks regardless.
pub(crate) const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
/// Left inset of the reading pane's text.
const INDENT: &str = " ";
/// Narrowest the body wraps to; below this a word per line reads worse
/// than overflowing.
const MIN_BODY_W: usize = 20;
/// The list column's share of the modal, and its floor.
const LIST_PCT: u16 = 38;
const MIN_LIST_W: u16 = 24;
/// Lines one wheel notch scrolls the reading pane.
const WHEEL_LINES: i32 = 3;

/// One open issue, as `gh issue list` reports it. The body rides the list
/// — one call paints the whole reading pane — and only the comments are a
/// second, per-row call ([`detail`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub author: String,
    /// RFC 3339, as GitHub gives it.
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    /// The description, verbatim markdown; the reading pane renders it
    /// (the MARKDOWN module) with GitHub's comment rule that a newline is
    /// a line break, like a pull request's.
    pub body: String,
}

impl Issue {
    /// Row text: `#15 title`, the shape the OPEN PRS rows use.
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            format!("#{}", self.number)
        } else {
            format!("#{} {}", self.number, self.title)
        }
    }

    /// What a launch carries from the row to the DAEMON.
    pub fn launch_ref(&self) -> IssueRef {
        IssueRef {
            url: self.url.clone(),
            number: self.number,
            title: self.title.clone(),
        }
    }
}

/// The issue a QUICK PROMPT or AGENT PRESET launch is for: enough to name
/// it in the box's title, cut a branch for it, write the default task, and
/// send the URL the DAEMON persists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub url: String,
    pub number: u64,
    pub title: String,
}

impl IssueRef {
    /// The task sent when the box is submitted empty: the issue itself.
    pub fn default_task(&self) -> String {
        if self.title.trim().is_empty() {
            format!("Fix GitHub issue #{} ({})", self.number, self.url)
        } else {
            format!(
                "Fix GitHub issue #{}: {} ({})",
                self.number,
                self.title.trim(),
                self.url
            )
        }
    }
}

/// One thing somebody said on an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub author: String,
    /// RFC 3339.
    pub at: String,
    pub body: String,
}

/// The per-row second call: the issue's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub url: String,
    pub comments: Vec<IssueComment>,
}

/// What `gh issue list` last said about a project, kept for the session so
/// reopening the modal paints at once.
#[derive(Debug, Clone)]
pub struct IssueList {
    pub list: Vec<Issue>,
    pub at: std::time::Instant,
}

/// When a project's list is next owed in the background, and why: the
/// steady [`REFRESH`] once the repo has proved it has issues (`backoff`
/// `None`), or the doubling step an empty or failed answer left it on —
/// the open pull requests' shape, kept apart from [`IssueList`] because a
/// failed first ask arms a beat without landing a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuesBeat {
    pub due: std::time::Instant,
    pub backoff: Option<std::time::Duration>,
}

/// What a debounced comments fetch needs: which issue, and the checkout to
/// run `gh` from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingIssueDetail {
    pub url: String,
    pub number: u64,
    pub dir: PathBuf,
}

/// A finished `gh` call, back on the loop. `None` is "couldn't ask".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuesAnswer {
    List {
        project: ProjectId,
        list: Option<Vec<Issue>>,
    },
    Detail {
        url: String,
        detail: Option<IssueDetail>,
    },
    /// `gh issue comment` finished; `posted` false is "couldn't post". The
    /// box's state rides along so a refusal can bring it back with the
    /// text, on the modal's row.
    Comment {
        view: IssuesView,
        issue: IssueRef,
        text: String,
        posted: bool,
    },
    /// `gh issue edit` finished: the text GitHub now holds, or why it
    /// refused — the first line `gh` printed.
    Edited {
        project: ProjectId,
        url: String,
        number: u64,
        outcome: Result<IssueText, String>,
    },
}

/// The modal's own state. The rows live on the [`App`] (`issues`, keyed by
/// project) so a fetch that lands after the modal closed still paints the
/// next open; this holds only the cursor, the reading pane's scroll, and
/// the rects the mouse hit-tests against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuesView {
    pub project: ProjectId,
    /// The project's row name, for the list's title.
    pub project_name: String,
    /// The checkout `gh` runs from.
    pub dir: PathBuf,
    /// Cursor into the project's list.
    pub selected: usize,
    /// Top visible line of the reading pane.
    pub scroll: u16,
    /// The reading pane's height and total line count as of the last draw,
    /// for paging and clamping.
    pub view_height: u16,
    pub body_lines: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// The list rows and the reading pane, for wheel and click routing.
    pub list_area: Rect,
    pub body_area: Rect,
    /// The `↗ open in browser` BUTTON on the reading pane's top border
    /// (`ui::browser_button`), for the click and the pointer resting on
    /// it; `Rect::default()` — no point inside — while there is none.
    pub browser_area: Rect,
    /// The list's live filter: the rows narrow to the fuzzy matches of
    /// `#15 title`, best first ([`visible_rows`]), as every letter lands.
    /// Empty shows every row in the list's order.
    pub query: TextInput,
    /// Where the cursor sat among the visible rows as of the last draw:
    /// the follow-window's anchor, and what a click's row math counts
    /// from.
    pub cursor_row: usize,
    /// `Ctrl+e`: the reading pane turned into the editor for the row under the
    /// cursor, until Enter has saved or Esc has dropped it. Boxed: the
    /// view rides a `Comment` answer, and two text fields would make that
    /// variant several times the others' size.
    pub editor: Option<Box<IssueEditor>>,
}

impl IssuesView {
    pub fn new(project: ProjectId, project_name: String, dir: PathBuf) -> Self {
        Self {
            project,
            project_name,
            dir,
            selected: 0,
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            browser_area: Rect::default(),
            query: TextInput::new(),
            cursor_row: 0,
            editor: None,
        }
    }

    /// First visible row of the list's stateless follow-window, over the
    /// rows the filter leaves.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor_row, height)
    }

    pub fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
    }
}

/// Which of the editor's two fields has the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Title,
    Body,
}

impl EditField {
    /// Tab order, wrapping — two fields, so the other one.
    pub fn next(self) -> EditField {
        match self {
            EditField::Title => EditField::Body,
            EditField::Body => EditField::Title,
        }
    }
}

/// An issue's title and description as GitHub holds them: what the editor
/// opens on, what a save sends, and what lands back on the row once
/// GitHub has taken it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueText {
    pub title: String,
    pub body: String,
}

/// `Ctrl+e` in the modal: the reading pane as a form for the issue under the
/// cursor — the title on one line, the description in a box under it —
/// that `Enter` sends to GitHub as one `gh issue edit`. It lives on the
/// [`IssuesView`] rather than as an overlay of its own: the list stays up
/// on the left, and Esc puts the reading pane back with nothing to
/// rebuild. The keys are the PRESET EDITOR's: Tab / ↑↓ between the fields,
/// Shift+Enter / Ctrl+J for a line in the description, Enter to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEditor {
    pub url: String,
    pub number: u64,
    pub title: TextInput,
    pub body: TextInput,
    pub field: EditField,
    /// What the form opened on: an unchanged Enter closes without a call.
    pub original: IssueText,
    /// `gh issue edit` is in flight: the form holds every key but Esc
    /// until the answer lands, so the text is still there if GitHub says
    /// no.
    pub saving: bool,
    /// Why the last Enter went nowhere — a blank title, `gh`'s complaint —
    /// shown on the frame until the next edit.
    pub notice: Option<String>,
    /// The title row and the description box, written back during draw
    /// so a click can move the caret between them.
    pub title_area: Rect,
    pub body_area: Rect,
}

impl IssueEditor {
    /// The form prefilled from the row, caret at the end of the title.
    pub fn new(issue: &Issue) -> Self {
        Self {
            url: issue.url.clone(),
            number: issue.number,
            title: TextInput::with_text(issue.title.clone()),
            body: TextInput::multiline_with_text(issue.body.clone()),
            field: EditField::Title,
            original: IssueText {
                title: issue.title.clone(),
                body: issue.body.clone(),
            },
            saving: false,
            notice: None,
            title_area: Rect::default(),
            body_area: Rect::default(),
        }
    }

    /// What Enter would send: the title trimmed, the description as typed.
    pub fn text(&self) -> IssueText {
        IssueText {
            title: self.title.trim().to_string(),
            body: self.body.as_str().to_string(),
        }
    }

    /// The field under the caret.
    pub fn field_mut(&mut self) -> &mut TextInput {
        match self.field {
            EditField::Title => &mut self.title,
            EditField::Body => &mut self.body,
        }
    }

    /// The field under the caret, to read.
    pub fn field_ref(&self) -> &TextInput {
        match self.field {
            EditField::Title => &self.title,
            EditField::Body => &self.body,
        }
    }
}

// ---- gh ----

async fn gh(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .current_dir(dir);
    let out = match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(_)) | Err(_) => return None,
    };
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Ask `gh` for every open issue on `dir`'s repo, newest first. `None` is
/// "couldn't ask" and is kept apart from `Some(vec![])`, the real answer
/// "nothing is open": the caller keeps the last good list over a failed
/// call. `gh issue list` leaves pull requests out on its own.
pub async fn list(dir: &Path) -> Option<Vec<Issue>> {
    let limit = LIST_LIMIT.to_string();
    let out = gh(
        dir,
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,url,title,author,createdAt,updatedAt,labels,body",
        ],
    )
    .await?;
    parse_list(&out)
}

/// Ask `gh` for one issue's conversation. `number` picks it, so any
/// checkout of the repo will do.
pub async fn detail(dir: &Path, number: u64) -> Option<IssueDetail> {
    let number = number.to_string();
    let out = gh(dir, &["issue", "view", &number, "--json", "url,comments"]).await?;
    parse_detail(&out)
}

/// Post `body` as a comment on issue `number`, as the `gh` user. The body
/// goes down stdin (`--body-file -`), never argv: a comment can be long,
/// and one opening with `-` must not read as a flag. True when `gh` exited
/// clean; anything else — no `gh`, not logged in, a network that stalled
/// past [`TIMEOUT`] — is "couldn't post". A `gh` still running at the
/// timeout is killed with the future, not left behind.
pub async fn comment(dir: &Path, number: u64, body: &str) -> bool {
    comment_via("gh", dir, number, body).await
}

/// [`comment`] through `program`: `gh` in the app, a script on disk in the
/// tests, since the real thing would post.
async fn comment_via(
    program: impl AsRef<std::ffi::OsStr>,
    dir: &Path,
    number: u64,
    body: &str,
) -> bool {
    use tokio::io::AsyncWriteExt;
    let number = number.to_string();
    let mut child = match tokio::process::Command::new(program)
        .args(["issue", "comment", &number, "--body-file", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(dir)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let Some(mut stdin) = child.stdin.take() else {
        return false;
    };
    let body = body.to_string();
    let feed = async move {
        stdin.write_all(body.as_bytes()).await?;
        stdin.shutdown().await
    };
    // Feed and wait together: a `gh` that exits before reading (bad auth)
    // would otherwise leave the write blocked on a closed pipe.
    let run = async {
        let (_, status) = tokio::join!(feed, child.wait());
        status.map(|s| s.success()).unwrap_or(false)
    };
    tokio::time::timeout(TIMEOUT, run).await.unwrap_or(false)
}

/// Send one issue a new title and description, as the `gh` user. Unlike
/// the reads this wants `gh`'s complaint, not just its silence: the first
/// line it printed is what the form shows when GitHub refuses. The
/// description goes down stdin (`--body-file -`) as a comment's does; the
/// title rides argv as `--title=…`, one token, so one opening with `-`
/// can't read as a flag either.
pub async fn edit(dir: &Path, number: u64, text: &IssueText) -> Result<(), String> {
    edit_via("gh", dir, number, text).await
}

/// [`edit`] through `program`: `gh` in the app, a script on disk in the
/// tests, since the real thing would edit.
async fn edit_via(
    program: impl AsRef<std::ffi::OsStr>,
    dir: &Path,
    number: u64,
    text: &IssueText,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let number = number.to_string();
    let title = format!("--title={}", text.title);
    let mut child = tokio::process::Command::new(program)
        .args(["issue", "edit", &number, &title, "--body-file", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .current_dir(dir)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't run gh: {e}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err("couldn't feed gh".into());
    };
    let body = text.body.clone();
    let feed = async move {
        stdin.write_all(body.as_bytes()).await?;
        stdin.shutdown().await
    };
    // Feed and wait together, as a comment does: a `gh` that exits before
    // reading (bad auth) would otherwise leave the write blocked on a
    // closed pipe.
    let run = async {
        let (_, out) = tokio::join!(feed, child.wait_with_output());
        match out {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(complaint(&out.stderr)),
            Err(e) => Err(format!("gh failed: {e}")),
        }
    };
    tokio::time::timeout(TIMEOUT, run)
        .await
        .unwrap_or_else(|_| Err("gh timed out".into()))
}

/// The first line `gh` printed that says anything, as a shell would have
/// shown it; a silent refusal gets a stock one.
fn complaint(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(|| "gh refused the edit".to_string(), str::to_string)
}

fn str_at(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

fn login(author: Option<&serde_json::Value>) -> String {
    author
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `v["url"]`, only when a browser could open it — the row leads straight
/// to one, and the text originates outside nebula.
fn web_url(v: &serde_json::Value) -> Option<String> {
    let url = v.get("url")?.as_str()?.to_string();
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

/// Parse `gh issue list --json …` — a bare array. A row with no number or
/// no openable URL drops out rather than failing the list; a payload that
/// isn't an array is a miss.
fn parse_list(json: &str) -> Option<Vec<Issue>> {
    let rows = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let rows = rows.as_array()?;
    Some(
        rows.iter()
            .filter_map(|v| {
                let url = web_url(v)?;
                Some(Issue {
                    number: v.get("number")?.as_u64()?,
                    url,
                    title: str_at(v, "title"),
                    author: login(v.get("author")),
                    created_at: str_at(v, "createdAt"),
                    updated_at: str_at(v, "updatedAt"),
                    labels: v
                        .get("labels")
                        .and_then(|l| l.as_array())
                        .map(|labels| {
                            labels
                                .iter()
                                .map(|l| str_at(l, "name"))
                                .filter(|n| !n.is_empty())
                                .collect()
                        })
                        .unwrap_or_default(),
                    body: str_at(v, "body"),
                })
            })
            .collect(),
    )
}

fn parse_detail(json: &str) -> Option<IssueDetail> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let url = web_url(&v)?;
    let mut comments: Vec<IssueComment> = v
        .get("comments")
        .and_then(|c| c.as_array())
        .map(|list| {
            list.iter()
                .map(|c| IssueComment {
                    author: login(c.get("author")),
                    at: str_at(c, "createdAt"),
                    body: str_at(c, "body"),
                })
                .collect()
        })
        .unwrap_or_default();
    // RFC 3339 UTC stamps sort lexicographically into chronological order.
    comments.sort_by(|a, b| a.at.cmp(&b.at));
    Some(IssueDetail { url, comments })
}

/// `2026-09-10` out of an RFC 3339 stamp; empty when there is none.
fn day(stamp: &str) -> &str {
    stamp.split('T').next().unwrap_or_default()
}

// ---- opening, fetching, landing ----

/// The hotkey: the ISSUES MODAL for the selected PROJECT. Every panel has
/// one selected, so this works from any row; only a machine with no
/// project has nothing to list.
pub(crate) fn open_issues(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        app.flash = Some("issues: select a project first".into());
        return;
    };
    let mut view = IssuesView::new(
        project.id.clone(),
        project.name.clone(),
        project.repo_path.clone(),
    );
    view.selected = clamp_selection(0, list_len(app, &project.id));
    app.overlay = Some(Overlay::Issues(view));
    // A list the prefetch landed moments ago is the answer; an older one
    // paints now while a fresh copy lands underneath.
    if !is_fresh(app, &project.id) {
        request_list(app, project.id, project.repo_path);
    }
    schedule_detail(app);
    app.dirty = true;
}

fn list_len(app: &App, project: &ProjectId) -> usize {
    app.issues.get(project).map_or(0, |l| l.list.len())
}

/// Is there a filter to apply — text in the row beyond whitespace?
fn has_query(view: &IssuesView) -> bool {
    view.query.split_whitespace().next().is_some()
}

/// The rows the filter leaves, top to bottom: indices into `list`, each
/// with the matched char positions of its `#15 title` (lit when drawn);
/// every row in list order with nothing typed. Worked out afresh on every
/// call rather than kept — a repo's open issues are a screenful — so it
/// can never go stale against the list.
fn visible_rows(query: &str, list: &[Issue]) -> Vec<(usize, Vec<usize>)> {
    let labels: Vec<String> = list.iter().map(|issue| issue.label()).collect();
    crate::fuzzy::rank(query, labels.iter().map(String::as_str))
}

/// The row under the cursor, as an index into `list`: `selected` while
/// the filter shows it, else the filter's best match — a refresh may have
/// moved the cursor's issue under a row the filter hides — and `selected`
/// clamped onto the list with nothing typed. None with no row to be on:
/// an empty list, or a filter nothing matches.
fn cursor_index(view: &IssuesView, list: &[Issue]) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    if !has_query(view) {
        return Some(clamp_selection(view.selected as i64, list.len()));
    }
    let visible = visible_rows(&view.query, list);
    if visible.iter().any(|(i, _)| *i == view.selected) {
        Some(view.selected)
    } else {
        visible.first().map(|(i, _)| *i)
    }
}

/// Put the modal back as it was — the box a comment was typed in stood in
/// for it — on the same row, clamped in case the list moved underneath,
/// with the row's comments asked for as landing on it would.
pub(crate) fn reopen(app: &mut App, mut view: IssuesView) {
    view.selected = clamp_selection(view.selected as i64, list_len(app, &view.project));
    app.overlay = Some(Overlay::Issues(view));
    schedule_detail(app);
    app.dirty = true;
}

/// Ask `gh` for the project's open issues, off the loop. Skipped while
/// one is in flight — a repaint must never stack `gh` processes. A repo
/// that isn't on disk is a miss noted without spending a process, and
/// left to the backoff — the checkout can come back. Without the loop's
/// sender installed (the unit tests) nothing is asked.
fn request_list(app: &mut App, project: ProjectId, dir: PathBuf) {
    if app.issues_inflight.contains(&project) {
        return;
    }
    if !dir.is_dir() {
        arm_beat(app, &project, false);
        app.issues_failed.insert(project);
        return;
    }
    let Some(tx) = app.issues_tx.clone() else {
        return;
    };
    app.issues_inflight.insert(project.clone());
    tokio::spawn(async move {
        let list = list(&dir).await;
        let _ = tx.send(IssuesAnswer::List { project, list });
    });
}

// ---- prefetching ----

/// Arm the debounced background ask for the selected project's open
/// issues; the loop fires it once the cursor has rested there
/// ([`PREFETCH_DEBOUNCE`]). Run wherever the selected project changes —
/// the project switch, the startup restore — so `i` finds the rows there.
pub(crate) fn schedule_prefetch(app: &mut App) {
    app.pending_issues_prefetch = app
        .selected_project()
        .map(|p| (p.id.clone(), std::time::Instant::now() + PREFETCH_DEBOUNCE));
}

/// The debounce elapsed: ask for the project the cursor settled on, when
/// it is still the selected one and its beat says so. Disarms first, so a
/// `gh` that never answers can't re-fire on every loop turn.
pub(crate) fn fire_prefetch(app: &mut App) {
    let Some((project, _)) = app.pending_issues_prefetch.take() else {
        return;
    };
    if app.selected_project().is_some_and(|p| p.id == project) {
        ask_selected_if_due(app);
    }
}

/// The git tick: re-ask the selected project's list once its beat has
/// passed — the steady [`REFRESH`] while it has issues, the backoff while
/// it hasn't — and ask for a project never asked about at all, so a
/// selection that landed without a project switch (the first project
/// added, the tree arriving) is prefetched too.
pub(crate) fn refresh_selected(app: &mut App) {
    ask_selected_if_due(app);
}

/// `Shift+R` on the grid, reload from GitHub: ask for the selected
/// project's open issues now, past the beat and a miss already
/// remembered — the modal's `Ctrl+r` for the list, from outside it.
pub(crate) fn reload_selected(app: &mut App) {
    let Some((project, dir)) = app
        .selected_project()
        .map(|p| (p.id.clone(), p.repo_path.clone()))
    else {
        return;
    };
    app.issues_failed.remove(&project);
    request_list(app, project, dir);
}

fn ask_selected_if_due(app: &mut App) {
    let Some((project, dir)) = app
        .selected_project()
        .map(|p| (p.id.clone(), p.repo_path.clone()))
    else {
        return;
    };
    if prefetch_due(app, &project) {
        request_list(app, project, dir);
    }
}

/// The git tick, with PR & ISSUE COUNTS on: ask for the open issues of one
/// project the cursor is *not* on, off the loop — the pass that keeps
/// every PROJECTS PANEL row's count warm without the user visiting the
/// project, the open pull requests' `sweep_open_prs` for issues. One
/// process per tick at most ([`sweep_target`]); the answer lands on
/// `App::issues_tx` like the selected project's own, which stays on its
/// faster beat.
pub(crate) fn sweep_others(app: &mut App) {
    let Some((project, dir)) = sweep_target(app) else {
        return;
    };
    request_list(app, project, dir);
}

/// The project the sweep should spend this tick on, if any: the first
/// project, in row order, that isn't selected, isn't in
/// flight, and was never asked, or whose list is older than
/// [`SWEEP_REFRESH`] — or whose own beat has run out, when that backoff is
/// the longer wait (a repo with nothing open, or no `gh`, keeps its
/// doubling step; a list that landed is on the two-minute [`REFRESH`],
/// which the sweep's slower beat overrides).
pub(crate) fn sweep_target(app: &App) -> Option<(ProjectId, PathBuf)> {
    let selected = app.selected_project().map(|p| p.id.clone());
    let now = std::time::Instant::now();
    app.project_rows()
        .into_iter()
        .map(|i| &app.tree.projects[i])
        .filter(|p| Some(&p.id) != selected.as_ref() && !app.issues_inflight.contains(&p.id))
        .find(|p| match app.issues_due.get(&p.id) {
            Some(beat) => {
                let listed_recently = app
                    .issues
                    .get(&p.id)
                    .is_some_and(|l| now < l.at + SWEEP_REFRESH);
                now >= beat.due && !listed_recently
            }
            None => true,
        })
        .map(|p| (p.id.clone(), p.repo_path.clone()))
}

/// Whether the background should ask for this project now: not while an
/// answer is in flight, not before the timer the last answer armed, and
/// always for a project never asked about.
pub(crate) fn prefetch_due(app: &App, project: &ProjectId) -> bool {
    if app.issues_inflight.contains(project) {
        return false;
    }
    match app.issues_due.get(project) {
        Some(beat) => std::time::Instant::now() >= beat.due,
        None => true,
    }
}

/// A list that landed within [`FRESH`]: the modal opens on it as it is.
fn is_fresh(app: &App, project: &ProjectId) -> bool {
    app.issues
        .get(project)
        .is_some_and(|l| l.at.elapsed() < FRESH)
}

/// Arm the next background ask after an answer: the steady beat when the
/// repo has open issues, a doubling backoff otherwise — from the floor
/// when the last answer had rows, so one flaky call after a good one
/// costs thirty seconds, not a doubled steady beat.
fn arm_beat(app: &mut App, project: &ProjectId, found: bool) {
    let (step, backoff) = if found {
        (REFRESH, None)
    } else {
        let step = match app.issues_due.get(project).and_then(|b| b.backoff) {
            Some(prev) => (prev * 2).min(RECHECK_MAX),
            None => RECHECK_MIN,
        };
        (step, Some(step))
    };
    app.issues_due.insert(
        project.clone(),
        IssuesBeat {
            due: std::time::Instant::now() + step,
            backoff,
        },
    );
}

/// Arm (or disarm) the debounced comments fetch for the issue in focus
/// ([`issue_in_focus`]: the modal's row, or the PROJECT ISSUES GROUP row
/// under the Worktrees cursor). An issue already fetched, in flight, or
/// known unanswerable arms nothing. Landing on a row rewinds the reading
/// pane.
pub(crate) fn schedule_detail(app: &mut App) {
    let pending = issue_in_focus(app).and_then(|(issue, dir)| {
        let url = issue.url.clone();
        if app.issue_detail.contains_key(&url)
            || app.issue_detail_inflight.contains(&url)
            || app.issue_detail_failed.contains(&url)
        {
            return None;
        }
        Some(PendingIssueDetail {
            url,
            number: issue.number,
            dir,
        })
    });
    app.pending_issue_detail = pending.map(|p| (p, std::time::Instant::now() + DETAIL_DEBOUNCE));
}

/// Fire the debounced fetch. Disarms first, so a `gh` that never answers
/// can't re-fire on every loop turn.
pub(crate) fn lookup_detail(app: &mut App) {
    let Some((pending, _)) = app.pending_issue_detail.take() else {
        return;
    };
    if !pending.dir.is_dir() {
        app.issue_detail_failed.insert(pending.url);
        app.dirty = true;
        return;
    }
    let Some(tx) = app.issues_tx.clone() else {
        return;
    };
    app.issue_detail_inflight.insert(pending.url.clone());
    tokio::spawn(async move {
        let detail = detail(&pending.dir, pending.number).await;
        let _ = tx.send(IssuesAnswer::Detail {
            url: pending.url,
            detail,
        });
    });
}

/// A `gh` answer landed. A list replaces the project's rows — keeping the
/// cursor on the issue it was on, by URL, so a refresh that retired a row
/// above it does not slide the selection — and a failed list keeps the
/// last good one, or says so when there is none; either way the next
/// background ask is armed off it. Comments replace whatever the pane
/// showed for the URL; a failed fetch is remembered so the pane says so
/// instead of spinning.
pub(crate) fn land_answer(app: &mut App, answer: IssuesAnswer) {
    match answer {
        IssuesAnswer::List { project, list } => {
            app.issues_inflight.remove(&project);
            arm_beat(app, &project, list.as_ref().is_some_and(|l| !l.is_empty()));
            match list {
                Some(list) => {
                    let cursor_url = match &app.overlay {
                        Some(Overlay::Issues(view)) if view.project == project => app
                            .issues
                            .get(&project)
                            .and_then(|l| l.list.get(cursor_index(view, &l.list)?))
                            .map(|i| i.url.clone()),
                        _ => None,
                    };
                    app.issues_failed.remove(&project);
                    let len = list.len();
                    let position = cursor_url
                        .as_ref()
                        .and_then(|url| list.iter().position(|i| &i.url == url));
                    app.issues.insert(
                        project.clone(),
                        IssueList {
                            list,
                            at: std::time::Instant::now(),
                        },
                    );
                    if let Some(Overlay::Issues(view)) = &mut app.overlay {
                        if view.project == project {
                            view.selected = match position {
                                Some(i) => i,
                                None => clamp_selection(view.selected as i64, len),
                            };
                        }
                    }
                    schedule_detail(app);
                }
                None => {
                    if !app.issues.contains_key(&project) {
                        app.issues_failed.insert(project);
                    }
                }
            }
        }
        IssuesAnswer::Detail { url, detail } => {
            app.issue_detail_inflight.remove(&url);
            match detail {
                Some(detail) => {
                    app.issue_detail.insert(url, detail);
                }
                None => {
                    app.issue_detail_failed.insert(url);
                }
            }
        }
        IssuesAnswer::Comment {
            view,
            issue,
            text,
            posted,
        } => {
            app.issue_comment_inflight.remove(&issue.url);
            if posted {
                // The conversation the pane has is one comment short now:
                // forget it, and the cursor resting on the row reads it
                // again, with the new comment in.
                app.issue_detail.remove(&issue.url);
                app.issue_detail_failed.remove(&issue.url);
                app.flash = Some(format!("comment posted on #{}", issue.number));
                schedule_detail(app);
            } else {
                app.flash = Some(format!(
                    "couldn't post the comment on #{} — is gh logged in?",
                    issue.number
                ));
                // The box comes back with the text for a retry — unless
                // something else has been opened over the modal meanwhile,
                // which the flash must not interrupt.
                if matches!(&app.overlay, None | Some(Overlay::Issues(_))) {
                    bring_box_back(app, view, issue, text);
                }
            }
        }
        IssuesAnswer::Edited {
            project,
            url,
            number,
            outcome,
        } => match outcome {
            Ok(text) => {
                // The row the pane reads from carries the new text at
                // once; the refresh underneath makes it GitHub's copy.
                if let Some(row) = app
                    .issues
                    .get_mut(&project)
                    .and_then(|l| l.list.iter_mut().find(|i| i.url == url))
                {
                    row.title = text.title;
                    row.body = text.body;
                }
                if let Some(Overlay::Issues(view)) = &mut app.overlay {
                    if view
                        .editor
                        .as_ref()
                        .is_some_and(|e| e.saving && e.url == url)
                    {
                        view.editor = None;
                    }
                }
                app.flash = Some(format!("issue #{number} updated"));
                if let Some(dir) = app
                    .tree
                    .projects
                    .iter()
                    .find(|p| p.id == project)
                    .map(|p| p.repo_path.clone())
                {
                    request_list(app, project, dir);
                }
            }
            Err(why) => {
                // The form that sent it shows why and keeps the text; one
                // that has since gone gets the footer.
                let mut told = false;
                if let Some(Overlay::Issues(view)) = &mut app.overlay {
                    if let Some(editor) = &mut view.editor {
                        if editor.saving && editor.url == url {
                            editor.saving = false;
                            editor.notice = Some(why.clone());
                            told = true;
                        }
                    }
                }
                if !told {
                    app.flash = Some(format!("couldn't update issue #{number}: {why}"));
                }
            }
        },
    }
    app.dirty = true;
}

// ---- commenting ----

/// `Ctrl+c`: the comment box for the issue under the cursor. The box replaces
/// the modal; Enter posts and comes back to it, Esc just comes back.
fn open_comment_for_selected(app: &mut App) {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return;
    };
    let view = view.clone();
    let Some((issue, _)) = selected_issue(app) else {
        app.flash = Some("no issue selected".into());
        return;
    };
    crate::event_loop::open_prompt(
        app,
        crate::app::PromptKind::IssueComment {
            view,
            issue: issue.launch_ref(),
        },
    );
}

/// The comment box again, with `text` typed back in.
fn bring_box_back(app: &mut App, view: IssuesView, issue: IssueRef, text: String) {
    crate::event_loop::reopen_prompt_with(
        app,
        crate::app::PromptKind::IssueComment { view, issue },
        text,
    );
}

/// Enter in the comment box: the modal comes back on its row at once and
/// the comment goes to GitHub off the loop — the pane says it is on its
/// way until [`land_answer`] hears back. A checkout that isn't on disk
/// can't run `gh`: the box comes back with the text and says so. Without
/// the loop's sender installed (the unit tests) nothing is posted.
pub(crate) fn post_comment(app: &mut App, view: IssuesView, issue: IssueRef, text: String) {
    let dir = view.dir.clone();
    if !dir.is_dir() {
        app.flash = Some(format!(
            "couldn't post the comment on #{}: the checkout isn't on disk",
            issue.number
        ));
        bring_box_back(app, view, issue, text);
        return;
    }
    reopen(app, view.clone());
    let Some(tx) = app.issues_tx.clone() else {
        return;
    };
    app.issue_comment_inflight.insert(issue.url.clone());
    app.flash = Some(format!("posting a comment on #{}…", issue.number));
    let number = issue.number;
    tokio::spawn(async move {
        let posted = comment(&dir, number, &text).await;
        let _ = tx.send(IssuesAnswer::Comment {
            view,
            issue,
            text,
            posted,
        });
    });
}

/// `Ctrl+r` in the modal: ask for the list again now, and the selected issue's
/// comments over the cached copy. The rows stay until the answer lands.
fn refresh(app: &mut App) {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    app.issues_failed.remove(&project);
    request_list(app, project, dir);
    if let Some((issue, dir)) = selected_issue(app) {
        if !app.issue_detail_inflight.contains(&issue.url) {
            app.issue_detail_failed.remove(&issue.url);
            app.pending_issue_detail = Some((
                PendingIssueDetail {
                    url: issue.url.clone(),
                    number: issue.number,
                    dir,
                },
                std::time::Instant::now(),
            ));
        }
    }
    app.flash = Some("refreshing issues…".into());
    app.dirty = true;
}

/// The issue under the cursor and the checkout to ask `gh` from, while the
/// modal is up and the list has rows.
fn selected_issue(app: &App) -> Option<(Issue, PathBuf)> {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return None;
    };
    let list = app.issues.get(&view.project)?.list.as_slice();
    let issue = list.get(cursor_index(view, list)?)?;
    Some((issue.clone(), view.dir.clone()))
}

/// The URL of the issue under the cursor, for the browser.
fn selected_url(app: &App) -> Option<String> {
    selected_issue(app).map(|(issue, _)| issue.url)
}

/// The issue whose comments the pane wants: the modal's row while the
/// modal is up, else the PROJECT ISSUES GROUP row under the Worktrees
/// cursor, asked from the project's checkout.
fn issue_in_focus(app: &App) -> Option<(Issue, PathBuf)> {
    if matches!(app.overlay, Some(Overlay::Issues(_))) {
        return selected_issue(app);
    }
    let issue = app.selected_worktree_issue()?.clone();
    let dir = app.selected_project()?.repo_path.clone();
    Some((issue, dir))
}

/// Move the cursor to `index` (clamped): the pane rewinds and the row's
/// comments are asked for once the cursor rests.
fn select(app: &mut App, index: i64) {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let len = app.issues.get(&view.project).map_or(0, |l| l.list.len());
    let next = clamp_selection(index, len);
    if next != view.selected {
        view.selected = next;
        view.scroll = 0;
    }
    schedule_detail(app);
    app.dirty = true;
}

/// ↑/↓, the wheel: the cursor `delta` rows through the visible ones —
/// the filter's matches while one is typed — clamped at either end.
fn step(app: &mut App, delta: i64) {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return;
    };
    let list = app
        .issues
        .get(&view.project)
        .map_or(&[][..], |l| l.list.as_slice());
    let Some(current) = cursor_index(view, list) else {
        return;
    };
    let visible = visible_rows(&view.query, list);
    let at = visible.iter().position(|(i, _)| *i == current).unwrap_or(0) as i64;
    let next = clamp_selection(at + delta, visible.len());
    if let Some((index, _)) = visible.get(next) {
        select(app, *index as i64);
    }
}

/// The filter's text changed: the cursor goes to its best match — the
/// pane rewinds onto it and its comments are asked for, as any move does
/// — or stays where it is once nothing is typed, so the row just found
/// keeps the cursor after Esc has cleared the letters that found it. A
/// filter nothing matches moves nothing: the list says so, the pane has
/// no row to read, and the next letter or Backspace decides.
fn query_changed(app: &mut App) {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return;
    };
    let list = app
        .issues
        .get(&view.project)
        .map_or(&[][..], |l| l.list.as_slice());
    let target = if has_query(view) {
        visible_rows(&view.query, list).first().map(|(i, _)| *i)
    } else {
        cursor_index(view, list)
    };
    match target {
        Some(index) => select(app, index as i64),
        None => schedule_detail(app),
    }
    app.dirty = true;
}

/// Esc: the filter cleared, the cursor staying on the row it was on.
fn clear_query(app: &mut App) {
    if let Some(Overlay::Issues(view)) = &mut app.overlay {
        view.query.clear();
    }
    query_changed(app);
}

// ---- editing ----

/// `Ctrl+e`: turn the reading pane into the editor for the issue under the
/// cursor, prefilled from the row. A list with no rows has nothing to
/// edit, and says so where the launch keys do.
fn open_editor(app: &mut App) {
    let Some((issue, _)) = selected_issue(app) else {
        app.flash = Some("no issue selected".into());
        return;
    };
    if let Some(Overlay::Issues(view)) = &mut app.overlay {
        view.editor = Some(Box::new(IssueEditor::new(&issue)));
    }
}

/// Enter in the editor: a blank title is refused on the spot, an
/// unchanged form closes without a call, and anything else goes to
/// `gh issue edit` off the loop with the form held until the answer
/// lands. Without the loop's sender installed (the unit tests) nothing
/// is sent and the form stays as it is.
fn save_editor(app: &mut App) {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(editor) = &mut view.editor else {
        return;
    };
    if editor.saving {
        return;
    }
    let text = editor.text();
    if text.title.is_empty() {
        editor.notice = Some("the issue needs a title".into());
        return;
    }
    if text == editor.original {
        view.editor = None;
        app.flash = Some("issue unchanged".into());
        return;
    }
    if !dir.is_dir() {
        editor.notice = Some("the checkout isn't on disk — gh has nowhere to run".into());
        return;
    }
    let Some(tx) = app.issues_tx.clone() else {
        return;
    };
    editor.saving = true;
    editor.notice = None;
    let (url, number) = (editor.url.clone(), editor.number);
    tokio::spawn(async move {
        let outcome = edit(&dir, number, &text).await.map(|()| text);
        let _ = tx.send(IssuesAnswer::Edited {
            project,
            url,
            number,
            outcome,
        });
    });
}

/// Keys while the editor is up: the form's own first — Tab / ↑↓ between
/// the two fields, Shift+Enter / Ctrl+J for a line in the description,
/// Enter to save, Esc to put the reading pane back unsaved — then the
/// field's LINE EDITOR keys. A save in flight holds every key but Esc.
fn handle_editor_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let Some(editor) = &mut view.editor else {
        return;
    };
    let mut save = false;
    match key.code {
        KeyCode::Esc => view.editor = None,
        _ if editor.saving => {}
        KeyCode::Tab | KeyCode::BackTab => editor.field = editor.field.next(),
        // ↓/↑ walk the description's lines first; past its last (or
        // first) line, and from the one-line title, they move between
        // the two fields.
        KeyCode::Down => {
            if !editor.field_mut().handle_key(&key).consumed() {
                editor.field = EditField::Body;
            }
        }
        KeyCode::Up => {
            if !editor.field_mut().handle_key(&key).consumed() {
                editor.field = EditField::Title;
            }
        }
        // A line break — Shift+Enter, Option+Enter or Ctrl+J, as in the
        // task editor — is the description's own (the `_` arm below).
        // Asked for on the title, which has no second line, it steps down
        // into the description rather than saving under the user.
        _ if editor.field == EditField::Title && TextInput::is_newline_key(&key) => {
            editor.field = EditField::Body;
        }
        KeyCode::Enter if !editor.field_ref().takes_newline(&key) => save = true,
        _ => {
            if editor.field_mut().handle_key(&key).changed() {
                editor.notice = None;
            }
        }
    }
    if save {
        save_editor(app);
    }
    app.dirty = true;
}

/// A bracketed paste while the editor is up lands in the field under the
/// caret — lines kept in the description, flattened in the title — and
/// otherwise in the filter, as one line, narrowing the rows as typing it
/// would. True whenever the modal is up: the filter is always live.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return false;
    };
    if view.editor.is_none() {
        view.query.insert_str(text);
        query_changed(app);
        return true;
    }
    let Some(editor) = &mut view.editor else {
        return false;
    };
    if editor.saving {
        return true;
    }
    editor.notice = None;
    // The title is one line, the description keeps the paste's line
    // breaks: each field knows which it is.
    editor.field_mut().insert_str(text);
    true
}

/// The footer's key line for the modal: the form's keys while it is up,
/// the reader's otherwise.
pub(crate) fn footer_hint(view: &IssuesView) -> &'static str {
    if view.editor.is_some() {
        "Tab/↑↓: field  ⇧Enter/^J: newline  Enter: save to GitHub  Esc: cancel edit"
    } else {
        "type to filter  ↑/↓ ^n/^p: issue  PgUp/PgDn ^d/^u: read  Enter: prompt an agent  ⇧Tab: preset  ^e: edit  ^c/^y: comment  ^o: browser  ^r: refresh  Esc: clear / close"
    }
}

// ---- launching ----

/// Where a launch from the modal lands: the project's ROOT WORKTREE —
/// which every project has whether the panel shows it or not — as every
/// new session's box does, never the checkout of the card under the
/// cursor; or, with the `quick_prompt_new_worktree` SETTING on, a fresh
/// worktree named after the issue. `Ctrl+N` in the box flips between the
/// two.
fn launch_target(app: &App, project: &ProjectId, issue: &IssueRef) -> Option<QuickTarget> {
    if crate::config::Config::load().quick_prompt_new_worktree {
        let taken = app.project_branches(project);
        return Some(QuickTarget::NewWorktree {
            project: project.clone(),
            branch: crate::branch_name::issue_name(issue.number, &issue.title, &taken),
        });
    }
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.project_id == project && w.is_main)
        .map(|w| QuickTarget::Worktree(w.id.clone()))
}

/// The launch a row describes: the `quick_prompt_kind` SETTING's harness
/// and defaults, aimed at [`launch_target`], carrying the issue.
fn launch_for_selected(app: &mut App) -> Option<QuickLaunch> {
    let Some(Overlay::Issues(view)) = &app.overlay else {
        return None;
    };
    let project = view.project.clone();
    let Some((issue, _)) = selected_issue(app) else {
        app.flash = Some("no issue selected".into());
        return None;
    };
    let issue = issue.launch_ref();
    let Some(target) = launch_target(app, &project, &issue) else {
        app.flash = Some("issues: the project has no worktree to launch into".into());
        return None;
    };
    Some(QuickLaunch::from_config(target, &crate::config::Config::load()).with_issue(Some(issue)))
}

/// `Enter`: the QUICK PROMPT for the issue. The box goes up over the
/// modal, which stays on screen under it: Esc puts the modal back on the
/// row (`QuickLaunch::under`), and the launch closes it onto the new
/// session's card.
fn open_prompt_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_box(app, launch.with_under(under));
    }
}

/// `Shift+Tab`: pick one of the saved AGENT PRESETS for the issue. The picker
/// goes up over the modal, as `Enter`'s box does, and hands its pick to that
/// same box with the preset applied — still standing on the modal. Esc (or a
/// click outside the list) puts the modal back on the row.
fn open_preset_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_preset_picker(
            app,
            QuickReturn {
                launch: launch.with_under(under),
                text: String::new(),
                from_box: false,
            },
        );
    }
}

/// The launch the PROJECT ISSUES GROUP row under the Worktrees cursor
/// describes — the modal's for that row: the `quick_prompt_kind`
/// SETTING's harness aimed at [`launch_target`], carrying the issue.
fn launch_for_row(app: &mut App) -> Option<QuickLaunch> {
    let issue = app.selected_worktree_issue()?.launch_ref();
    let project = app.selected_project()?.id.clone();
    let Some(target) = launch_target(app, &project, &issue) else {
        app.flash = Some("issues: the project has no worktree to launch into".into());
        return None;
    };
    Some(QuickLaunch::from_config(target, &crate::config::Config::load()).with_issue(Some(issue)))
}

/// `p` on a PROJECT ISSUES GROUP row: the QUICK PROMPT for that issue —
/// what `Enter` in the modal opens on the same row.
pub(crate) fn open_prompt_for_row(app: &mut App) {
    if let Some(launch) = launch_for_row(app) {
        crate::quick_prompt::open_box(app, launch);
    }
}

/// `e` on a PROJECT ISSUES GROUP row: an AGENT PRESET on that issue.
pub(crate) fn open_preset_for_row(app: &mut App) {
    if let Some(launch) = launch_for_row(app) {
        crate::quick_prompt::open_preset_picker(
            app,
            QuickReturn {
                launch,
                text: String::new(),
                from_box: false,
            },
        );
    }
}

/// `Ctrl+o`, and a click on the reading pane's `↗ open in browser` button
/// (`HitTarget::ModalBrowser`): the issue under the cursor in the
/// browser, through the very `event_loop::open_link` a card's `⇧V` and
/// `⇧I` run — the footer says where it went, or that it could not.
/// Nothing under the cursor opens nothing. INPUT PARITY: the key and the
/// click end in the same state.
pub(crate) fn open_in_browser(app: &mut App, out: &mut Vec<ClientRequest>) {
    if let Some(url) = selected_url(app) {
        crate::event_loop::open_link(app, &url, out);
    }
}

// ---- keys and mouse ----

/// Keys in the ISSUES MODAL. While the editor is up they are all its
/// ([`handle_editor_key`]). Otherwise the filter is always live, so
/// letters type — the modal's own hotkey and `q` among them — and the
/// verbs are chords; only Esc closes, once the filter is clear.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    if matches!(&app.overlay, Some(Overlay::Issues(v)) if v.editor.is_some()) {
        handle_editor_key(app, key);
        return;
    }
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let half = (view.view_height / 2).max(1) as i32;
    let page = view.view_height.max(1) as i32;
    match key.code {
        // Two-stage escape, like every fuzzy overlay: a typed filter is
        // cleared before the second Esc closes the modal.
        KeyCode::Esc if !view.query.is_empty() => clear_query(app),
        KeyCode::Esc => app.overlay = None,
        // Shift+↑/↓ scroll the pane a line; ↑/↓ walk the rows the filter
        // leaves, Ctrl+n/p mirroring them.
        KeyCode::Down if shift => view.scroll_by(1),
        KeyCode::Up if shift => view.scroll_by(-1),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
        KeyCode::Char('n') if ctrl => step(app, 1),
        KeyCode::Char('p') if ctrl => step(app, -1),
        // The reading pane scrolls on the DIFF VIEWER's keys. Ctrl+u is
        // the line editor's kill-to-start while something is typed; only
        // with an empty filter does it scroll.
        KeyCode::Char('d') if ctrl => view.scroll_by(half),
        KeyCode::Char('u') if ctrl && view.query.is_empty() => view.scroll_by(-half),
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        // The launches are the QUICK PROMPT box's own keys: Enter prompts,
        // Shift+Tab picks a preset (a shifted Tab under the kitty protocol
        // is the same key).
        KeyCode::Enter => open_prompt_for_selected(app),
        KeyCode::BackTab => open_preset_for_selected(app),
        KeyCode::Tab if shift => open_preset_for_selected(app),
        // The AGENT PRESETS list's edit chord.
        KeyCode::Char('e') if ctrl => open_editor(app),
        // `Ctrl+y` is the grid's `y` (reply) as a chord, the letters being
        // the filter's.
        KeyCode::Char('c') | KeyCode::Char('y') if ctrl => open_comment_for_selected(app),
        KeyCode::Char('o') if ctrl => open_in_browser(app, out),
        KeyCode::Char('r') if ctrl => refresh(app),
        // Everything else feeds the always-live fuzzy filter, which edits
        // like a terminal line (see text_input).
        _ => {
            if view.query.handle_key(&key).changed() {
                query_changed(app);
            }
        }
    }
    app.dirty = true;
}

/// Mouse in the ISSUES MODAL: the wheel moves the cursor over the rows
/// the filter leaves and scrolls the reading pane over it, a click on a
/// row selects it (a launch is `Enter`, not a click — the row is
/// something to read first), and a click outside closes (`overlay_close`);
/// everything else is swallowed. While the editor is up a click moves the
/// caret between its two fields and nothing else — the list and the wheel
/// would drop the draft under the user; Esc is the way out.
pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    if let Some(Overlay::Issues(IssuesView {
        editor: Some(editor),
        ..
    })) = &mut app.overlay
    {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            if editor.title_area.contains(mouse_pos) {
                editor.field = EditField::Title;
            } else if editor.body_area.contains(mouse_pos) {
                editor.field = EditField::Body;
            }
        }
        app.dirty = true;
        return;
    }
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let over_body = view.body_area.contains(mouse_pos);
    let on_button = view.browser_area.contains(mouse_pos);
    match mouse.kind {
        MouseEventKind::ScrollUp if over_body => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::ScrollDown if over_body => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => step(app, -1),
        MouseEventKind::ScrollDown => step(app, 1),
        // The `↗ open in browser` button, before the rows: the very open
        // `Ctrl+o` runs.
        MouseEventKind::Down(MouseButton::Left) if on_button => open_in_browser(app, out),
        MouseEventKind::Down(MouseButton::Left) => {
            let list = view.list_area;
            let first = view.window_start(list.height as usize);
            // The row math counts the filter's matches, not the whole list.
            let visible = visible_rows(
                &view.query,
                app.issues
                    .get(&view.project)
                    .map_or(&[][..], |l| l.list.as_slice()),
            );
            if let Some(row) = crate::list_hit::row_at(list, first, visible.len(), mouse_pos) {
                select(app, visible[row].0 as i64);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

// ---- drawing ----

/// The reading pane as styled lines: headline, state row, description,
/// then the conversation once it has landed — and, while a comment of
/// yours is on its way to it, a line saying so.
pub fn lines(
    issue: &Issue,
    detail: Option<&IssueDetail>,
    comments_failed: bool,
    posting: bool,
    width: usize,
    th: Theme,
) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(INDENT.len() + 1).max(MIN_BODY_W);
    let dim = Style::default().fg(th.dim);
    let muted = Style::default().fg(th.muted);
    let mut out: Vec<Line<'static>> = Vec::new();

    out.push(fit(
        vec![
            Span::styled(format!("{INDENT}#{} ", issue.number), dim),
            Span::styled(
                issue.title.clone(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
        ],
        width,
    ));
    let mut meta = vec![
        Span::styled(INDENT.to_string(), dim),
        Span::styled(
            "open".to_string(),
            Style::default().fg(th.ok).add_modifier(Modifier::BOLD),
        ),
    ];
    if !issue.author.is_empty() {
        meta.push(Span::styled(format!(" · {}", issue.author), muted));
    }
    let opened = day(&issue.created_at);
    if !opened.is_empty() {
        meta.push(Span::styled(format!(" · {opened}"), dim));
    }
    out.push(fit(meta, width));
    if !issue.labels.is_empty() {
        out.push(fit(
            vec![
                Span::styled(INDENT.to_string(), dim),
                Span::styled(issue.labels.join(" · "), Style::default().fg(th.warn)),
            ],
            width,
        ));
    }
    out.push(Line::from(""));

    if issue.body.trim().is_empty() {
        out.push(Line::from(Span::styled(
            format!("{INDENT}(no description)"),
            dim,
        )));
    } else {
        out.extend(markdown::indent(
            markdown::render(issue.body.trim_end(), body_w, Breaks::Hard, muted, th),
            INDENT,
        ));
    }

    out.push(Line::from(""));
    match detail {
        Some(detail) if detail.comments.is_empty() => {
            out.push(Line::from(Span::styled(
                format!("{INDENT}── no comments ──"),
                dim,
            )));
        }
        Some(detail) => {
            out.push(fit(
                vec![Span::styled(
                    format!(
                        "{INDENT}── {} comment{} ──",
                        detail.comments.len(),
                        if detail.comments.len() == 1 { "" } else { "s" }
                    ),
                    dim,
                )],
                width,
            ));
            for c in &detail.comments {
                out.push(Line::from(""));
                let mut head = vec![Span::styled(
                    format!("{INDENT}{}", c.author),
                    Style::default().fg(th.accent),
                )];
                let at = day(&c.at);
                if !at.is_empty() {
                    head.push(Span::styled(format!(" · {at}"), dim));
                }
                out.push(fit(head, width));
                out.extend(markdown::indent(
                    markdown::render(
                        c.body.trim_end(),
                        body_w.saturating_sub(2),
                        Breaks::Hard,
                        muted,
                        th,
                    ),
                    &format!("{INDENT}  "),
                ));
            }
        }
        None if comments_failed => {
            out.push(Line::from(Span::styled(
                format!("{INDENT}── couldn't read the comments ──"),
                dim,
            )));
        }
        None => {
            out.push(Line::from(Span::styled(
                format!("{INDENT}── reading the comments… ──"),
                dim,
            )));
        }
    }
    if posting {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!("{INDENT}── posting your comment… ──"),
            dim,
        )));
    }
    out
}

/// The ISSUES MODAL: the list down the left, the reading pane on the right.
/// `backdrop` draws it as the layer under a QUICK PROMPT box opened from it
/// (`QuickLaunch::under`): dim frames and an unfocused cursor row, the box
/// in front having the eye.
pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &IssuesView, th: Theme, backdrop: bool) {
    // The list holds the keys unless the editor or a box over the modal does.
    let list_focused = view.editor.is_none() && !backdrop;
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(SPLIT_PANE_LAYOUT_MIN));
    let [list_a, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    let rows: Vec<Issue> = app
        .issues
        .get(&view.project)
        .map(|l| l.list.clone())
        .unwrap_or_default();
    let inflight = app.issues_inflight.contains(&view.project);
    let failed = app.issues_failed.contains(&view.project);
    // The rows the filter leaves, and where the cursor sits among them.
    let visible = visible_rows(&view.query, &rows);
    let cursor = cursor_index(view, &rows);
    let cursor_row = cursor
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);

    // ---- left: the list ----
    // The count reads `matches/all` while a filter is on.
    let count = if has_query(view) {
        format!("{}/{}", visible.len(), rows.len())
    } else {
        rows.len().to_string()
    };
    let title = format!(
        "Issues — {} ({}{})",
        view.project_name,
        count,
        if inflight { ", refreshing…" } else { "" }
    );
    let block = panel_block(&title, list_focused, th).title_bottom(
        Line::from(Span::styled(
            " Enter: prompt  ⇧Tab: preset  ^e: edit  ^c: comment  ^o: browser  ^r: refresh ",
            Style::default().fg(th.dim),
        ))
        .left_aligned(),
    );
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    // The always-live filter on the list's first line, the rows under it.
    if let Some(query_area) = row_rect(list_inner, 0) {
        let line = search_line(&view.query, "type to filter…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let rows_area = Rect {
        y: list_inner.y.saturating_add(1),
        height: list_inner.height.saturating_sub(1),
        ..list_inner
    };
    if rows.is_empty() {
        let text = if failed {
            "couldn't list issues — is gh installed and logged in?"
        } else if inflight || app.issues_tx.is_some() && !app.issues.contains_key(&view.project) {
            "asking GitHub…"
        } else {
            "no open issues"
        };
        empty_list_row(f, rows_area, text, th);
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no issues match", th);
    }
    let start = window_start(cursor_row, rows_area.height as usize);
    for (row, (index, positions)) in visible.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(rows_area, row - start) else {
            break;
        };
        let issue = &rows[*index];
        let budget = (rows_area.width as usize).saturating_sub(2);
        // `#15 title` left, the day it was opened pinned right, dim; the
        // chars the filter matched lit.
        let opened = day(&issue.created_at).to_string();
        let opened_w = opened.chars().count();
        let text_budget = budget.saturating_sub(if opened_w > 0 { opened_w + 2 } else { 0 });
        let full = issue.label();
        let label = truncate(&full, text_budget);
        let positions = visible_positions(positions, &label, &full);
        let used = label.chars().count();
        let number = format!("#{} ", issue.number);
        let number_w = number.chars().count();
        let title = label.strip_prefix(&number).unwrap_or(&label).to_string();
        // The positions split where the number ends: the title's own
        // count from its first char.
        let split = positions.partition_point(|&p| p < number_w);
        let title_positions: Vec<usize> = positions[split..].iter().map(|p| p - number_w).collect();
        let mut spans = fuzzy_highlight_styled(
            &number,
            &positions[..split],
            Style::default().fg(th.dim),
            th,
        );
        spans.extend(fuzzy_highlight_styled(
            &title,
            &title_positions,
            Style::default(),
            th,
        ));
        if opened_w > 0 && used + opened_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - opened_w)));
            spans.push(Span::styled(opened, Style::default().fg(th.dim)));
        }
        render_row(f, row_area, spans, Some(*index) == cursor, list_focused, th);
    }

    // ---- right: the editor, while it is up ----
    if let Some(editor) = &view.editor {
        let (title_area, body_area, body_view) = draw_editor(f, body_a, editor, th);
        if let Some(Overlay::Issues(v)) = &mut app.overlay {
            v.area = area;
            v.list_area = rows_area;
            v.cursor_row = cursor_row;
            v.browser_area = Rect::default();
            if let Some(index) = cursor {
                v.selected = index;
            }
            if let Some(e) = &mut v.editor {
                e.title_area = title_area;
                e.body_area = body_area;
                if let Some(view) = body_view {
                    e.body.set_view(view);
                }
            }
        }
        return;
    }

    // ---- right: the reading pane ----
    let current = cursor.and_then(|i| rows.get(i));
    // The frame names the number; the headline inside carries the title.
    let body_title = match current {
        Some(issue) => format!("Issue #{}", issue.number),
        None => "Issue".to_string(),
    };
    let lines: Vec<Line> = match current {
        Some(issue) => lines(
            issue,
            app.issue_detail.get(&issue.url),
            app.issue_detail_failed.contains(&issue.url),
            app.issue_comment_inflight.contains(&issue.url),
            body_a.width.saturating_sub(2) as usize,
            th,
        ),
        None => Vec::new(),
    };
    let mut block = panel_block(&body_title, false, th);
    let body_inner = block.inner(body_a);
    let max_scroll = (lines.len() as u16).saturating_sub(body_inner.height.max(1));
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, lines.len()),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, body_a);
    // The `↗ open in browser` button over the top border, once the block
    // has drawn it — only with a row to open.
    let browser_area = match current {
        Some(_) => crate::ui::browser_button(
            f,
            body_a,
            (body_title.chars().count() + 2) as u16,
            app.hover_crumb == Some(HitTarget::ModalBrowser),
            th,
        ),
        None => Rect::default(),
    };
    let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
    f.render_widget(Paragraph::new(shown), body_inner);

    // Write-back (draw works on a clone): the rects the mouse hit-tests,
    // the pane's size for paging, and the clamped cursor and scroll.
    if let Some(Overlay::Issues(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = rows_area;
        v.cursor_row = cursor_row;
        v.body_area = body_inner;
        v.browser_area = browser_area;
        v.view_height = body_inner.height;
        v.body_lines = lines.len();
        // A cursor the filter had to move (see `cursor_index`) is settled
        // onto its row.
        if let Some(index) = cursor {
            v.selected = index;
        }
        v.scroll = scroll;
    }
}

/// The reading pane as the form: the title on the first row, the
/// description in a box under it, the frame's foot saying what Enter will
/// do — or why the last one did nothing, or that GitHub is being asked.
/// Returns the title row and the description box for click-to-focus, and
/// the view the description was drawn with while it has the caret.
fn draw_editor(
    f: &mut Frame,
    area: Rect,
    editor: &IssueEditor,
    th: Theme,
) -> (Rect, Rect, Option<TextView>) {
    let title = format!("Edit issue #{}", editor.number);
    let (foot, style) = match (&editor.notice, editor.saving) {
        (Some(notice), _) => (format!(" {notice} "), Style::default().fg(th.err)),
        (None, true) => (" saving… ".to_string(), Style::default().fg(th.warn)),
        (None, false) if area.width >= 60 => (
            " Tab: field  ⇧Enter: newline  Enter: save  Esc: cancel ".to_string(),
            Style::default().fg(th.dim),
        ),
        (None, false) => (
            " Enter: save  Esc: cancel ".to_string(),
            Style::default().fg(th.dim),
        ),
    };
    let block = panel_block(&title, true, th)
        .title_bottom(Line::from(Span::styled(foot, style)).left_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Row 0: the title, a one-line field.
    let mut title_area = Rect::default();
    if let Some(row) = row_rect(inner, 0) {
        title_area = row;
        let focused = editor.field == EditField::Title;
        let label = format!("{INDENT}Title  ");
        let label_style = if focused {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.muted)
        };
        let budget = (row.width as usize).saturating_sub(label.chars().count() + 1);
        let mut spans = vec![Span::styled(label, label_style)];
        if focused {
            spans.extend(input_spans(&editor.title, budget, th.accent, th));
        } else if editor.title.trim().is_empty() {
            spans.push(Span::styled("(required)", Style::default().fg(th.dim)));
        } else {
            spans.push(Span::raw(truncate(editor.title.as_str(), budget)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }

    // The description box, taking the rest of the pane.
    let box_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let mut body_area = Rect::default();
    let mut body_view = None;
    if box_area.height >= 3 && box_area.width >= 4 {
        body_area = box_area;
        let focused = editor.field == EditField::Body;
        let border = if focused { th.accent } else { th.dim };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .title(Span::styled(" Description ", Style::default().fg(border)));
        let box_inner = block.inner(box_area);
        f.render_widget(block, box_area);
        if focused {
            let (view, rows) = draw_multiline_input(f, &editor.body, box_inner, th);
            draw_scroll_marks(f, box_area, view, rows, th.dim);
            body_view = Some(view);
        } else if editor.body.trim().is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "(no description)",
                    Style::default().fg(th.dim),
                )),
                box_inner,
            );
        } else {
            f.render_widget(
                Paragraph::new(editor.body.as_str().to_string()).wrap(Wrap { trim: false }),
                box_inner,
            );
        }
    }
    (title_area, body_area, body_view)
}

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl IssueEditor {
    /// Whether Enter has anything to send.
    pub fn is_changed(&self) -> bool {
        self.text() != self.original
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, title: &str) -> Issue {
        Issue {
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            title: title.into(),
            author: "webdevcody".into(),
            created_at: "2026-09-10T12:00:00Z".into(),
            updated_at: "2026-09-11T12:00:00Z".into(),
            labels: vec!["bug".into(), "ui".into()],
            body: "Login bounces back to /.".into(),
        }
    }

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_a_gh_issue_list_payload_newest_first_as_given() {
        let issues = parse_list(
            r#"[
              {"number":15,"title":"Fix login redirect","url":"https://github.com/o/r/issues/15",
               "author":{"login":"webdevcody"},"createdAt":"2026-09-10T12:00:00Z",
               "updatedAt":"2026-09-11T12:00:00Z","labels":[{"name":"bug","color":"d73a4a"},{"name":"ui"}],
               "body":"Login bounces."},
              {"number":14,"title":"","url":"https://github.com/o/r/issues/14"},
              {"number":13,"url":"file:///etc/passwd"},
              {"url":"https://github.com/o/r/issues/12"}
            ]"#,
        )
        .expect("parsed");
        assert_eq!(
            issues.len(),
            2,
            "rows without a number or an http url drop out"
        );
        assert_eq!(issues[0].label(), "#15 Fix login redirect");
        assert_eq!(issues[0].author, "webdevcody");
        assert_eq!(issues[0].labels, ["bug", "ui"]);
        assert_eq!(issues[0].body, "Login bounces.");
        assert_eq!(
            issues[1].label(),
            "#14",
            "a missing title still names the issue"
        );
        assert!(issues[1].labels.is_empty());
        assert_eq!(parse_list("[]"), Some(vec![]), "an empty repo is an answer");
        assert!(parse_list("").is_none());
        assert!(parse_list("{}").is_none());
    }

    #[test]
    fn parses_a_gh_issue_view_payload_into_an_oldest_first_thread() {
        let d = parse_detail(
            r#"{"url":"https://github.com/o/r/issues/15","comments":[
              {"author":{"login":"kate"},"createdAt":"2026-09-11T08:00:00Z","body":"same here"},
              {"author":{"login":"steiza"},"createdAt":"2026-09-10T20:00:00Z","body":"repro?"}
            ]}"#,
        )
        .expect("parsed");
        assert_eq!(d.url, "https://github.com/o/r/issues/15");
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.comments[0].author, "steiza", "sorted by time");
        assert_eq!(d.comments[1].author, "kate");
        let bare = parse_detail(r#"{"url":"https://github.com/o/r/issues/1"}"#).expect("parsed");
        assert!(bare.comments.is_empty());
        assert!(parse_detail("{}").is_none());
    }

    #[test]
    fn the_launch_ref_and_its_default_task_name_the_issue() {
        let r = issue(15, "Fix login redirect").launch_ref();
        assert_eq!(r.number, 15);
        assert_eq!(
            r.default_task(),
            "Fix GitHub issue #15: Fix login redirect (https://github.com/o/r/issues/15)"
        );
        let untitled = issue(3, "  ").launch_ref();
        assert_eq!(
            untitled.default_task(),
            "Fix GitHub issue #3 (https://github.com/o/r/issues/3)"
        );
    }

    #[test]
    fn the_pane_leads_with_the_headline_then_body_then_comments() {
        let i = issue(15, "Fix login redirect");
        let waiting = text(&lines(&i, None, false, false, 60, Theme::default()));
        assert!(waiting.starts_with(" #15 Fix login redirect"), "{waiting}");
        assert!(
            waiting.contains("open · webdevcody · 2026-09-10"),
            "{waiting}"
        );
        assert!(waiting.contains("bug · ui"), "{waiting}");
        assert!(waiting.contains("Login bounces back to /."), "{waiting}");
        assert!(waiting.contains("reading the comments…"), "{waiting}");

        let failed = text(&lines(&i, None, true, false, 60, Theme::default()));
        assert!(failed.contains("couldn't read the comments"), "{failed}");

        let detail = IssueDetail {
            url: i.url.clone(),
            comments: vec![IssueComment {
                author: "kate".into(),
                at: "2026-09-11T08:00:00Z".into(),
                body: "same here".into(),
            }],
        };
        let read = text(&lines(
            &i,
            Some(&detail),
            false,
            false,
            60,
            Theme::default(),
        ));
        assert!(read.contains("── 1 comment ──"), "{read}");
        assert!(read.contains("kate · 2026-09-11"), "{read}");
        assert!(read.contains("same here"), "{read}");
        let quiet = IssueDetail {
            url: i.url.clone(),
            comments: vec![],
        };
        let none = text(&lines(&i, Some(&quiet), false, false, 60, Theme::default()));
        assert!(none.contains("── no comments ──"), "{none}");
    }

    /// The body reaches `gh` whole, down stdin — `--body-file -`, so a
    /// comment longer than a pipe buffer, or one opening with `-`, arrives
    /// as typed — and a `gh` that quits without reading it (bad auth) is a
    /// clean "couldn't post", not a write stuck on a closed pipe. So is a
    /// `gh` that isn't there.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_comment_body_goes_down_stdin_and_a_refusal_is_a_miss() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, sh: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{sh}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let records = script(
            "gh",
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\ncat > \"$(dirname \"$0\")/body\"",
        );
        let body = format!("-- starts like a flag\n{}", "x".repeat(200_000));
        assert!(comment_via(&records, dir.path(), 15, &body).await);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args")).unwrap(),
            "issue\ncomment\n15\n--body-file\n-\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("body")).unwrap(),
            body
        );

        let refuses = script("gh-refuses", "exit 1");
        assert!(!comment_via(&refuses, dir.path(), 15, &body).await);
        let missing = dir.path().join("gh-missing");
        assert!(!comment_via(&missing, dir.path(), 15, "hi").await);
    }

    /// While a comment of yours is on its way the pane says so, under
    /// whatever the conversation shows.
    #[test]
    fn the_pane_says_when_a_comment_is_posting() {
        let i = issue(15, "Fix login redirect");
        let quiet = IssueDetail {
            url: i.url.clone(),
            comments: vec![],
        };
        let posting = text(&lines(&i, Some(&quiet), false, true, 60, Theme::default()));
        assert!(posting.contains("── no comments ──"), "{posting}");
        assert!(
            posting.ends_with("── posting your comment… ──"),
            "{posting}"
        );
        let idle = text(&lines(&i, Some(&quiet), false, false, 60, Theme::default()));
        assert!(!idle.contains("posting"), "{idle}");
    }

    /// `Ctrl+c` opens the comment box for the issue under the cursor, carrying
    /// the modal so Esc and Enter can put it back on the row; with no rows
    /// it says so and stays.
    #[test]
    fn c_opens_the_comment_box_for_the_selected_issue() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        handle_key(&mut app, c, &mut Vec::new());
        assert!(
            matches!(&app.overlay, Some(Overlay::Issues(_))),
            "no rows: the modal stays"
        );
        assert_eq!(app.flash.as_deref(), Some("no issue selected"));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![issue(15, "a"), issue(14, "Fix login redirect")]),
            },
        );
        select(&mut app, 1);
        handle_key(&mut app, c, &mut Vec::new());
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("c should open the comment box, got {:?}", app.overlay);
        };
        assert!(prompt.is_multiline(), "a comment is rarely one line");
        assert_eq!(prompt.title, "Comment on issue #14 · Fix login redirect");
        assert!(
            prompt.label.contains("gh issue comment"),
            "{}",
            prompt.label
        );
        let crate::app::PromptKind::IssueComment { view, issue } = &prompt.kind else {
            panic!("{:?}", prompt.kind);
        };
        assert_eq!(view.selected, 1, "the row the box came from");
        assert_eq!(issue.number, 14);
    }

    /// Enter posts off the loop and puts the modal back on its row; a
    /// checkout that isn't on disk can't run `gh`, so the box comes back
    /// with the text instead of losing it.
    #[test]
    fn posting_puts_the_modal_back_and_a_missing_checkout_returns_the_box() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![issue(15, "a"), issue(14, "b")]),
            },
        );
        let mut view = IssuesView::new(project.clone(), "demo".into(), std::env::temp_dir());
        view.selected = 1;
        let fourteen = issue(14, "b").launch_ref();
        post_comment(&mut app, view.clone(), fourteen.clone(), "lgtm".into());
        assert!(
            matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 1),
            "the modal is back on its row: {:?}",
            app.overlay
        );
        // No sender installed: nothing was posted, nothing is in flight.
        assert!(app.issue_comment_inflight.is_empty());

        view.dir = "/nonexistent/nebula-issue-comment".into();
        post_comment(&mut app, view, fourteen, "lgtm".into());
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("the box should come back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "lgtm");
        assert!(matches!(
            prompt.kind,
            crate::app::PromptKind::IssueComment { .. }
        ));
        assert!(
            app.flash.as_deref().unwrap().contains("isn't on disk"),
            "{:?}",
            app.flash
        );
    }

    /// A posted comment forgets the conversation the pane had — one
    /// comment short now — so the row reads it again; a refused post
    /// brings the box back with the text, unless something else is up.
    #[test]
    fn a_comment_answer_rereads_the_conversation_or_brings_the_box_back() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        let view = IssuesView::new(project.clone(), "demo".into(), "/tmp/demo".into());
        app.overlay = Some(Overlay::Issues(view.clone()));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![issue(15, "a")]),
            },
        );
        let fifteen = issue(15, "a");
        land_answer(
            &mut app,
            IssuesAnswer::Detail {
                url: fifteen.url.clone(),
                detail: Some(IssueDetail {
                    url: fifteen.url.clone(),
                    comments: vec![],
                }),
            },
        );
        app.pending_issue_detail = None;
        let answer = |posted: bool| IssuesAnswer::Comment {
            view: view.clone(),
            issue: fifteen.launch_ref(),
            text: "lgtm".into(),
            posted,
        };

        app.issue_comment_inflight.insert(fifteen.url.clone());
        land_answer(&mut app, answer(true));
        assert!(!app.issue_comment_inflight.contains(&fifteen.url));
        assert!(
            !app.issue_detail.contains_key(&fifteen.url),
            "the conversation is one comment short: forgotten"
        );
        assert!(
            app.pending_issue_detail.is_some(),
            "…and asked for again as the cursor rests"
        );
        assert_eq!(app.flash.as_deref(), Some("comment posted on #15"));
        assert!(matches!(&app.overlay, Some(Overlay::Issues(_))));

        app.issue_comment_inflight.insert(fifteen.url.clone());
        land_answer(&mut app, answer(false));
        assert!(!app.issue_comment_inflight.contains(&fifteen.url));
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("a refused post brings the box back, got {:?}", app.overlay);
        };
        assert_eq!(prompt.input.as_str(), "lgtm");
        assert!(matches!(
            prompt.kind,
            crate::app::PromptKind::IssueComment { .. }
        ));
        assert!(
            app.flash.as_deref().unwrap().contains("couldn't post"),
            "{:?}",
            app.flash
        );

        // Something else up over the modal: the flash says, the box stays away.
        app.overlay = Some(Overlay::Help(Default::default()));
        land_answer(&mut app, answer(false));
        assert!(matches!(&app.overlay, Some(Overlay::Help(_))));
        assert!(app.flash.as_deref().unwrap().contains("couldn't post"));
    }

    #[test]
    fn no_rendered_line_overflows_the_pane() {
        let mut i = issue(
            15,
            "A very long title that goes on and on and on past the edge",
        );
        i.body = format!(
            "unbroken https://github.com/AgentSystemLabs/nebula/issues/15#issuecomment-{}",
            "1".repeat(80)
        );
        let detail = IssueDetail {
            url: i.url.clone(),
            comments: vec![IssueComment {
                author: "steiza".into(),
                at: "2026-09-11T08:00:00Z".into(),
                body: "a".repeat(200),
            }],
        };
        for w in [24usize, 40, 80] {
            for line in lines(&i, Some(&detail), false, false, w, Theme::default()) {
                let len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(len <= w, "width {w}: {len} cols in {line:?}");
            }
        }
    }

    /// A list landing keeps the cursor on the issue it was on, by URL —
    /// a refresh that retired a row above it must not slide the selection.
    #[test]
    fn a_landing_list_follows_the_cursor_by_url() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![issue(15, "a"), issue(14, "b"), issue(13, "c")]),
            },
        );
        select(&mut app, 2);
        assert!(matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 2));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![issue(15, "a"), issue(13, "c")]),
            },
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Issues(v)) if v.selected == 1),
            "#13 moved up a row and the cursor followed"
        );
        // A failed refresh keeps the rows; a failed first ask says so.
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: None,
            },
        );
        assert_eq!(app.issues[&project].list.len(), 2);
        assert!(!app.issues_failed.contains(&project));
        let other = ProjectId("p2".into());
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: other.clone(),
                list: None,
            },
        );
        assert!(app.issues_failed.contains(&other));
    }

    fn seed_project(app: &mut App, id: &str, dir: &str) -> ProjectId {
        let project = ProjectId(id.into());
        app.tree.projects.push(nebula_core::Project {
            id: project.clone(),
            name: id.into(),
            repo_path: dir.into(),
            sort_order: 0,
        });
        project
    }

    /// The git tick sweeps the other projects one per tick on a slow beat
    /// — never the selected one (it has its own), never one in flight, and
    /// only past its own backoff.
    #[test]
    fn the_sweep_visits_the_other_projects_one_per_tick() {
        let mut app = App::new();
        let p1 = seed_project(&mut app, "p1", "/nonexistent/nebula-issues-sweep-1");
        let p2 = seed_project(&mut app, "p2", "/nonexistent/nebula-issues-sweep-2");
        assert_eq!(
            app.selected_project().map(|p| p.id.clone()),
            Some(p1.clone())
        );
        let target = |app: &App| sweep_target(app).map(|(id, _)| id);
        assert_eq!(target(&app), Some(p2.clone()), "never asked: due");

        sweep_others(&mut app);
        assert!(
            app.issues_failed.contains(&p2),
            "not on disk: a miss without a process"
        );
        assert!(
            !app.issues_failed.contains(&p1),
            "the selected project is the prefetch's, never the sweep's"
        );
        assert_eq!(target(&app), None, "the miss armed its backoff");

        let now = std::time::Instant::now();
        app.issues.insert(
            p2.clone(),
            IssueList {
                list: vec![issue(1, "a")],
                at: now,
            },
        );
        app.issues_due.insert(
            p2.clone(),
            IssuesBeat {
                due: now,
                backoff: None,
            },
        );
        assert_eq!(
            target(&app),
            None,
            "answered just now: the sweep's beat is slower than REFRESH"
        );
        let stale = now
            .checked_sub(SWEEP_REFRESH + std::time::Duration::from_secs(1))
            .expect("machine up for minutes");
        app.issues.get_mut(&p2).unwrap().at = stale;
        assert_eq!(target(&app), Some(p2.clone()), "older than the beat");

        app.issues_due.get_mut(&p2).unwrap().due = now + RECHECK_MAX;
        assert_eq!(target(&app), None, "its own, longer backoff holds it");
        app.issues_due.get_mut(&p2).unwrap().due = now;

        app.issues_inflight.insert(p2.clone());
        assert_eq!(target(&app), None, "already in flight");
        app.issues_inflight.clear();
        assert_eq!(target(&app), Some(p2.clone()));

        app.issues_due.remove(&p1);
        app.issues.remove(&p1);
        assert_eq!(
            target(&app),
            Some(p2),
            "the selected project is never the sweep's, even unasked"
        );
    }

    /// Landing on a project arms the debounced prefetch for it, and firing
    /// it asks: a checkout that isn't on disk is a miss noted without a
    /// process, and the beat that miss arms keeps the tick from asking
    /// again at once.
    #[test]
    fn the_prefetch_arms_for_the_selected_project_and_fires_once() {
        let mut app = App::new();
        let project = seed_project(&mut app, "p1", "/nonexistent/nebula-issues-prefetch");
        assert!(prefetch_due(&app, &project), "never asked: due");
        schedule_prefetch(&mut app);
        let (armed, _) = app.pending_issues_prefetch.clone().expect("armed");
        assert_eq!(armed, project);
        assert!(app.issues_prefetch_delay().is_some());
        fire_prefetch(&mut app);
        assert!(
            app.pending_issues_prefetch.is_none(),
            "fires once, then disarms"
        );
        assert!(
            app.issues_failed.contains(&project),
            "not on disk: a miss without a process"
        );
        assert!(!prefetch_due(&app, &project), "the miss armed the backoff");
        assert_eq!(app.issues_due[&project].backoff, Some(RECHECK_MIN));
        // The tick asks nothing while the beat holds.
        app.issues_failed.clear();
        refresh_selected(&mut app);
        assert!(app.issues_failed.is_empty());
        // No project at all arms nothing.
        let mut empty = App::new();
        schedule_prefetch(&mut empty);
        assert!(empty.pending_issues_prefetch.is_none());
        assert!(empty.issues_prefetch_delay().is_none());
    }

    /// The beat: a list with rows settles on `REFRESH`, an empty or failed
    /// answer backs off by doubling to the ceiling, and rows again reset
    /// it — and nothing is due while an answer is in flight.
    #[test]
    fn the_beat_settles_on_refresh_and_backs_off_while_empty() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        fn land(app: &mut App, project: &ProjectId, list: Option<Vec<Issue>>) {
            app.issues_inflight.insert(project.clone());
            land_answer(
                app,
                IssuesAnswer::List {
                    project: project.clone(),
                    list,
                },
            );
        }
        let before = std::time::Instant::now();
        land(&mut app, &project, Some(vec![issue(15, "a")]));
        let beat = app.issues_due[&project];
        assert_eq!(beat.backoff, None);
        assert!(beat.due >= before + REFRESH, "steady beat");
        assert!(!prefetch_due(&app, &project));

        land(&mut app, &project, Some(vec![]));
        assert_eq!(app.issues_due[&project].backoff, Some(RECHECK_MIN));
        land(&mut app, &project, None);
        assert_eq!(app.issues_due[&project].backoff, Some(RECHECK_MIN * 2));
        for _ in 0..10 {
            land(&mut app, &project, None);
        }
        assert_eq!(app.issues_due[&project].backoff, Some(RECHECK_MAX));
        land(&mut app, &project, Some(vec![issue(15, "a")]));
        assert_eq!(app.issues_due[&project].backoff, None);

        app.issues_due.get_mut(&project).unwrap().due = std::time::Instant::now();
        assert!(prefetch_due(&app, &project), "the timer ran out");
        app.issues_inflight.insert(project.clone());
        assert!(!prefetch_due(&app, &project), "never while in flight");
    }

    /// The modal opens on a list that landed within `FRESH` without asking
    /// again; an older one is re-asked while its rows paint.
    #[test]
    fn a_list_that_just_landed_is_fresh_and_an_old_one_is_not() {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        assert!(!is_fresh(&app, &project), "nothing landed");
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(vec![]),
            },
        );
        assert!(is_fresh(&app, &project), "an empty answer is an answer");
        app.issues.get_mut(&project).unwrap().at = std::time::Instant::now()
            .checked_sub(FRESH + std::time::Duration::from_secs(1))
            .expect("the clock has run longer than FRESH");
        assert!(!is_fresh(&app, &project));
    }

    /// Comments land keyed by URL, and a miss is remembered so the pane
    /// says so instead of re-asking on every turn.
    #[test]
    fn comments_land_by_url_and_a_miss_is_remembered() {
        let mut app = App::new();
        let url = "https://github.com/o/r/issues/15".to_string();
        app.issue_detail_inflight.insert(url.clone());
        land_answer(
            &mut app,
            IssuesAnswer::Detail {
                url: url.clone(),
                detail: None,
            },
        );
        assert!(app.issue_detail_failed.contains(&url));
        assert!(!app.issue_detail_inflight.contains(&url));
        land_answer(
            &mut app,
            IssuesAnswer::Detail {
                url: url.clone(),
                detail: Some(IssueDetail {
                    url: url.clone(),
                    comments: vec![],
                }),
            },
        );
        assert!(app.issue_detail.contains_key(&url));
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn modal_with(rows: Vec<Issue>) -> (App, ProjectId) {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.overlay = Some(Overlay::Issues(IssuesView::new(
            project.clone(),
            "demo".into(),
            "/tmp/demo".into(),
        )));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project: project.clone(),
                list: Some(rows),
            },
        );
        (app, project)
    }

    fn editor(app: &App) -> Option<&IssueEditor> {
        match &app.overlay {
            Some(Overlay::Issues(v)) => v.editor.as_deref(),
            _ => None,
        }
    }

    fn editor_mut(app: &mut App) -> &mut IssueEditor {
        match &mut app.overlay {
            Some(Overlay::Issues(v)) => v.editor.as_deref_mut().expect("editing"),
            other => panic!("no issues modal: {other:?}"),
        }
    }

    /// `Ctrl+e` turns the pane into a form prefilled from the row, caret on
    /// the title; Esc puts the pane back with the draft dropped and the
    /// modal still up. An empty list has nothing to edit.
    #[test]
    fn shift_e_opens_the_editor_on_the_row_and_esc_drops_the_draft() {
        let (mut app, project) = modal_with(vec![issue(15, "Fix login redirect")]);
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        let e = editor(&app).expect("editing");
        assert_eq!(e.title.as_str(), "Fix login redirect");
        assert_eq!(e.body.as_str(), "Login bounces back to /.");
        assert_eq!(e.field, EditField::Title);
        assert!(!e.is_changed());
        handle_key(
            &mut app,
            key(KeyCode::Char('!'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).unwrap().is_changed());
        handle_key(
            &mut app,
            key(KeyCode::Esc, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).is_none(), "back to the reading pane");
        assert!(
            matches!(&app.overlay, Some(Overlay::Issues(_))),
            "the modal stays up"
        );
        assert_eq!(
            app.issues[&project].list[0].title, "Fix login redirect",
            "nothing sent"
        );
        // A plain `e` is the filter's, not the editor's.
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).is_none());
        assert_eq!(issues_view(&app).query.as_str(), "e");
        let (mut empty, _) = modal_with(vec![]);
        handle_key(
            &mut empty,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert!(editor(&empty).is_none());
        assert_eq!(empty.flash.as_deref(), Some("no issue selected"));
    }

    /// Tab moves the caret between the two fields and ↑/↓ do too once
    /// they have walked the description's lines, Shift+Enter and Ctrl+J
    /// break a line in the description — and step into it from the title,
    /// which has no second line — and a paste keeps its lines in the
    /// description while the title flattens them.
    #[test]
    fn the_editor_fields_take_the_form_keys() {
        let (mut app, _) = modal_with(vec![issue(15, "Fix login redirect")]);
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        handle_key(
            &mut app,
            key(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut Vec::new(),
        );
        let e = editor(&app).expect("a line break on the title is not a save");
        assert_eq!(e.title.as_str(), "Fix login redirect", "no line in a title");
        assert_eq!(e.field, EditField::Body, "it steps into the description");
        handle_key(
            &mut app,
            key(KeyCode::Tab, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().field, EditField::Title);
        handle_key(
            &mut app,
            key(KeyCode::Tab, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().field, EditField::Body);
        handle_key(
            &mut app,
            key(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut Vec::new(),
        );
        handle_key(
            &mut app,
            key(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert_eq!(
            editor(&app).unwrap().body.as_str(),
            "Login bounces back to /.\n\n"
        );
        // ↑ walks the description's three lines before it leaves the field.
        handle_key(
            &mut app,
            key(KeyCode::Up, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        let e = editor(&app).unwrap();
        assert_eq!((e.field, e.body.cursor_chars()), (EditField::Body, 25));
        handle_key(
            &mut app,
            key(KeyCode::Up, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        let e = editor(&app).unwrap();
        assert_eq!((e.field, e.body.cursor_chars()), (EditField::Body, 0));
        handle_key(
            &mut app,
            key(KeyCode::Up, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().field, EditField::Title);
        handle_key(
            &mut app,
            key(KeyCode::Down, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().field, EditField::Body);
        handle_key(
            &mut app,
            key(KeyCode::BackTab, KeyModifiers::SHIFT),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().field, EditField::Title);

        assert!(paste(&mut app, "a\nb"));
        assert_eq!(
            editor(&app).unwrap().title.as_str(),
            "Fix login redirecta b"
        );
        editor_mut(&mut app).field = EditField::Body;
        assert!(paste(&mut app, "c\r\nd"));
        // The walk above left the description's caret on its first
        // character, so the paste lands there — lines kept.
        assert!(editor(&app).unwrap().body.starts_with("c\nd"));
        let Some(Overlay::Issues(view)) = &mut app.overlay else {
            unreachable!()
        };
        view.editor = None;
        assert!(
            paste(&mut app, "x"),
            "with the form down, the paste is the filter's"
        );
        assert_eq!(issues_view(&app).query.as_str(), "x");
    }

    /// Enter refuses a blank title on the spot, closes an unchanged form
    /// without a call, and — without the loop's sender — leaves a changed
    /// one where it is.
    #[test]
    fn enter_validates_before_it_saves() {
        let (mut app, _) = modal_with(vec![issue(15, "Fix login redirect")]);
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        handle_key(
            &mut app,
            key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).unwrap().title.as_str(), "");
        handle_key(
            &mut app,
            key(KeyCode::Enter, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        let e = editor(&app).expect("refused, still editing");
        assert_eq!(e.notice.as_deref(), Some("the issue needs a title"));
        assert!(!e.saving);
        // Typing clears the notice.
        handle_key(
            &mut app,
            key(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).unwrap().notice.is_none());
        // Back to the original text: nothing to send.
        editor_mut(&mut app).title.set_text("Fix login redirect");
        handle_key(
            &mut app,
            key(KeyCode::Enter, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).is_none(), "an unchanged form just closes");
        assert_eq!(app.flash.as_deref(), Some("issue unchanged"));
        // A changed one with no sender installed stays put, unsent.
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        handle_key(
            &mut app,
            key(KeyCode::Char('!'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        handle_key(
            &mut app,
            key(KeyCode::Enter, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        let e = editor(&app).expect("still editing");
        assert!(!e.saving);
        assert_eq!(e.title.as_str(), "Fix login redirect!");
    }

    /// A save that GitHub took lands on the row and closes the form; one
    /// it refused keeps the form and its text, with the reason on it. A
    /// form opened since the send is not the one the answer is for.
    #[test]
    fn an_edit_landing_updates_the_row_or_keeps_the_form() {
        let (mut app, project) = modal_with(vec![issue(15, "Fix login redirect")]);
        let url = "https://github.com/o/r/issues/15".to_string();
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        let e = editor_mut(&mut app);
        e.title.set_text("Fix the login redirect");
        e.saving = true;
        // Keys wait on the answer; Esc would not.
        handle_key(
            &mut app,
            key(KeyCode::Char('?'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(
            editor(&app).unwrap().title.as_str(),
            "Fix the login redirect"
        );
        land_answer(
            &mut app,
            IssuesAnswer::Edited {
                project: project.clone(),
                url: url.clone(),
                number: 15,
                outcome: Err("HTTP 403: forbidden".into()),
            },
        );
        let e = editor(&app).expect("the form stays");
        assert!(!e.saving);
        assert_eq!(e.notice.as_deref(), Some("HTTP 403: forbidden"));
        assert_eq!(
            e.title.as_str(),
            "Fix the login redirect",
            "the text is kept"
        );
        assert_eq!(app.issues[&project].list[0].title, "Fix login redirect");

        editor_mut(&mut app).saving = true;
        land_answer(
            &mut app,
            IssuesAnswer::Edited {
                project: project.clone(),
                url: url.clone(),
                number: 15,
                outcome: Ok(IssueText {
                    title: "Fix the login redirect".into(),
                    body: "Bounces to /.".into(),
                }),
            },
        );
        assert!(editor(&app).is_none(), "saved: the reading pane is back");
        let row = &app.issues[&project].list[0];
        assert_eq!(row.title, "Fix the login redirect");
        assert_eq!(row.body, "Bounces to /.");
        assert_eq!(app.flash.as_deref(), Some("issue #15 updated"));

        // A form reopened meanwhile is left alone by a late answer.
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        land_answer(
            &mut app,
            IssuesAnswer::Edited {
                project: project.clone(),
                url,
                number: 15,
                outcome: Err("late".into()),
            },
        );
        let e = editor(&app).expect("still editing");
        assert!(e.notice.is_none());
        assert_eq!(
            app.flash.as_deref(),
            Some("couldn't update issue #15: late")
        );
    }

    /// The title rides argv as one `--title=` token and the description
    /// goes down stdin, however long and whatever it starts with; `gh`'s
    /// first stderr line is the refusal's reason, a silent one gets a
    /// stock reason, and a `gh` that isn't there says so.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_edit_sends_the_title_on_argv_and_the_body_down_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, sh: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{sh}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let records = script(
            "gh",
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\ncat > \"$(dirname \"$0\")/body\"",
        );
        let text = IssueText {
            title: "-- starts like a flag".into(),
            body: format!("-- so does this\n{}", "x".repeat(200_000)),
        };
        assert_eq!(edit_via(&records, dir.path(), 15, &text).await, Ok(()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args")).unwrap(),
            "issue\nedit\n15\n--title=-- starts like a flag\n--body-file\n-\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("body")).unwrap(),
            text.body
        );

        let refuses = script(
            "gh-refuses",
            "echo >&2\necho '  GraphQL: Resource not accessible' >&2\nexit 1",
        );
        assert_eq!(
            edit_via(&refuses, dir.path(), 15, &text).await,
            Err("GraphQL: Resource not accessible".into())
        );
        let silent = script("gh-silent", "exit 1");
        assert_eq!(
            edit_via(&silent, dir.path(), 15, &text).await,
            Err("gh refused the edit".into())
        );
        let missing = dir.path().join("gh-missing");
        let why = edit_via(&missing, dir.path(), 15, &text).await.unwrap_err();
        assert!(why.starts_with("couldn't run gh"), "{why}");
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::Issues(v)) = app.overlay.clone() else {
                panic!("no issues modal");
            };
            draw(f, app, &v, app.theme, false);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            handle_key(
                app,
                key(KeyCode::Char(c), KeyModifiers::NONE),
                &mut Vec::new(),
            );
        }
    }

    fn issues_view(app: &App) -> &IssuesView {
        match &app.overlay {
            Some(Overlay::Issues(v)) => v,
            other => panic!("expected the issues modal, got {other:?}"),
        }
    }

    fn cursor_number(app: &App) -> Option<u64> {
        selected_issue(app).map(|(issue, _)| issue.number)
    }

    /// Typing narrows the rows the moment the modal is up — no key to
    /// press first — to the fuzzy matches, best first, the cursor on the
    /// best with its comments asked for as any move's are, the count
    /// reading `matches/all`; the modal's own hotkey types too. ↑/↓ walk
    /// the matches alone and Ctrl+e edits the one found. The first Esc
    /// clears the filter, the cursor staying on the row it found, and the
    /// second closes.
    #[test]
    fn typing_filters_the_rows_at_once_and_esc_clears_before_closing() {
        let (mut app, _) = modal_with(vec![
            issue(15, "Fix login redirect"),
            issue(14, "Docs pass"),
            issue(13, "Login page"),
        ]);
        app.pending_issue_detail = None;
        assert!(footer_hint(issues_view(&app)).starts_with("type to filter"));
        type_str(&mut app, "login");
        assert_eq!(issues_view(&app).query.as_str(), "login");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(2/3)"), "{shot}");
        assert!(
            !shot.contains("#14"),
            "the docs row is filtered out:\n{shot}"
        );
        let found = cursor_number(&app).expect("a row under the cursor");
        assert_ne!(found, 14);
        assert!(
            app.pending_issue_detail.is_some(),
            "its comments are asked for"
        );

        // `i` types, rather than closing.
        handle_key(
            &mut app,
            key(KeyCode::Char('i'), KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(issues_view(&app).query.as_str(), "logini");
        assert!(matches!(&app.overlay, Some(Overlay::Issues(_))));
        handle_key(
            &mut app,
            key(KeyCode::Backspace, KeyModifiers::NONE),
            &mut Vec::new(),
        );

        let first = cursor_number(&app).unwrap();
        handle_key(
            &mut app,
            key(KeyCode::Down, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        let second = cursor_number(&app).unwrap();
        assert_ne!(first, second);
        assert_ne!(second, 14, "↓ walks the matches alone");
        handle_key(
            &mut app,
            key(KeyCode::Down, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert_eq!(
            cursor_number(&app),
            Some(second),
            "and stops at the last one"
        );
        // The verbs act on the row found: Ctrl+e edits it.
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert_eq!(editor(&app).expect("editing").number, second);
        handle_key(
            &mut app,
            key(KeyCode::Esc, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(editor(&app).is_none());
        assert_eq!(
            issues_view(&app).query.as_str(),
            "login",
            "leaving the editor keeps the filter"
        );

        // The first Esc clears the filter, the cursor staying put; the
        // second closes.
        handle_key(
            &mut app,
            key(KeyCode::Esc, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(matches!(&app.overlay, Some(Overlay::Issues(_))));
        assert!(issues_view(&app).query.is_empty());
        assert_eq!(cursor_number(&app), Some(second));
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(3)"), "{shot}");
        assert!(shot.contains("type to filter…"), "{shot}");
        handle_key(
            &mut app,
            key(KeyCode::Esc, KeyModifiers::NONE),
            &mut Vec::new(),
        );
        assert!(app.overlay.is_none());
    }

    /// A filter nothing matches empties the list and says so — nothing
    /// under the cursor, no comments asked for — and the row is back the
    /// moment the filter widens. Ctrl+u kills the typed filter, as in any
    /// line editor, and scrolls the pane only once there is none.
    #[test]
    fn a_filter_nothing_matches_says_so_and_leaves_the_cursor_put() {
        let (mut app, _) = modal_with(vec![
            issue(15, "Fix login redirect"),
            issue(14, "Docs pass"),
        ]);
        select(&mut app, 1);
        assert_eq!(cursor_number(&app), Some(14));
        type_str(&mut app, "fix");
        assert_eq!(
            cursor_number(&app),
            Some(15),
            "the cursor goes to the best match"
        );
        type_str(&mut app, "zzz");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("no issues match"), "{shot}");
        assert!(shot.contains("(0/2)"), "{shot}");
        assert_eq!(cursor_number(&app), None, "nothing under the cursor");
        assert!(app.pending_issue_detail.is_none());
        for _ in 0..3 {
            handle_key(
                &mut app,
                key(KeyCode::Backspace, KeyModifiers::NONE),
                &mut Vec::new(),
            );
        }
        assert_eq!(
            cursor_number(&app),
            Some(15),
            "the row is back as the filter widens"
        );
        if let Some(Overlay::Issues(v)) = &mut app.overlay {
            v.scroll = 3;
        }
        handle_key(
            &mut app,
            key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert!(
            issues_view(&app).query.is_empty(),
            "Ctrl+u kills the typed filter"
        );
        assert_eq!(issues_view(&app).scroll, 3, "and does not scroll the pane");
        assert_eq!(
            cursor_number(&app),
            Some(15),
            "the row found keeps the cursor"
        );
        handle_key(
            &mut app,
            key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert_eq!(
            issues_view(&app).scroll,
            0,
            "with nothing typed, it scrolls"
        );
    }

    /// A click on a row while a filter is typed picks that row — the row
    /// math counting the filter's matches, not the whole list — and the
    /// filter stays. A paste lands in the filter as one line.
    #[test]
    fn a_row_click_counts_the_matches_not_the_list() {
        let (mut app, project) = modal_with(vec![
            issue(15, "Fix login redirect"),
            issue(14, "Docs pass"),
            issue(13, "Login page"),
        ]);
        assert!(paste(&mut app, "log\nin"));
        assert_eq!(issues_view(&app).query.as_str(), "log in");
        for _ in 0..3 {
            handle_key(
                &mut app,
                key(KeyCode::Backspace, KeyModifiers::NONE),
                &mut Vec::new(),
            );
        }
        type_str(&mut app, "in");
        screen(&mut app, 120, 40);
        let list = issues_view(&app).list_area;
        // The second visible row: the second match, whichever it is.
        let at = Position::new(list.x + 1, list.y + 1);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut Vec::new());
        assert_eq!(
            issues_view(&app).query.as_str(),
            "login",
            "the filter is kept"
        );
        let picked = cursor_number(&app).unwrap();
        let visible = visible_rows("login", &app.issues[&project].list);
        assert_eq!(picked, app.issues[&project].list[visible[1].0].number);
        assert_ne!(picked, 14);
    }

    /// A refresh that retires the row under the cursor while a filter is
    /// typed lands the cursor on the filter's next match, never on a row
    /// the filter hides.
    #[test]
    fn a_refresh_under_a_filter_lands_on_a_visible_row() {
        let (mut app, project) = modal_with(vec![
            issue(15, "Fix login redirect"),
            issue(14, "Docs pass"),
            issue(13, "Login page"),
        ]);
        type_str(&mut app, "login");
        select(&mut app, 2);
        assert_eq!(cursor_number(&app), Some(13));
        land_answer(
            &mut app,
            IssuesAnswer::List {
                project,
                list: Some(vec![
                    issue(15, "Fix login redirect"),
                    issue(14, "Docs pass"),
                ]),
            },
        );
        assert_eq!(
            cursor_number(&app),
            Some(15),
            "not #14, which the filter hides"
        );
        screen(&mut app, 120, 40);
        assert_eq!(
            issues_view(&app).selected,
            0,
            "settled onto the row it shows"
        );
    }

    /// The verbs the letters used to be are chords now: Ctrl+r asks
    /// GitHub again, saying so in the footer, while the plain letters go
    /// to the filter.
    #[test]
    fn the_verb_chords_run_and_the_plain_letters_type() {
        let (mut app, _) = modal_with(vec![issue(15, "Fix login redirect")]);
        handle_key(
            &mut app,
            key(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        assert_eq!(app.flash.as_deref(), Some("refreshing issues…"));
        for letter in "roce".chars() {
            handle_key(
                &mut app,
                key(KeyCode::Char(letter), KeyModifiers::NONE),
                &mut Vec::new(),
            );
        }
        assert_eq!(issues_view(&app).query.as_str(), "roce");
        assert!(editor(&app).is_none());
        assert!(matches!(&app.overlay, Some(Overlay::Issues(_))));
    }

    /// `Shift+R` on the grid reloads the issues with the pull requests
    /// (`reload_selected`): the list is asked for now, past a beat that
    /// is not due.
    #[test]
    fn reload_asks_for_the_selected_projects_issues_past_the_beat() {
        let mut app = App::new();
        let project = seed_project(&mut app, "p1", "/nonexistent/nebula-issues-reload");
        app.issues_due.insert(
            project.clone(),
            IssuesBeat {
                due: std::time::Instant::now() + RECHECK_MAX,
                backoff: None,
            },
        );
        refresh_selected(&mut app);
        assert!(app.issues_failed.is_empty(), "the tick waits for the beat");
        reload_selected(&mut app);
        assert!(
            app.issues_failed.contains(&project),
            "asked at once: not on disk, so a miss without a process"
        );
        reload_selected(&mut App::new());
    }

    /// `Ctrl+y` comments as `Ctrl+c` does: the grid's reply key, as a
    /// chord because the letters are the filter's.
    #[test]
    fn ctrl_y_opens_the_comment_box_as_ctrl_c_does() {
        for letter in ['c', 'y'] {
            let (mut app, _) = modal_with(vec![issue(15, "Fix login redirect")]);
            handle_key(
                &mut app,
                key(KeyCode::Char(letter), KeyModifiers::CONTROL),
                &mut Vec::new(),
            );
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!(
                    "^{letter} should open the comment box, got {:?}",
                    app.overlay
                );
            };
            assert!(
                matches!(prompt.kind, crate::app::PromptKind::IssueComment { .. }),
                "{:?}",
                prompt.kind
            );
        }
    }

    /// The form paints in the reading pane's place — the list still on
    /// the left — and the frame's foot carries the keys, the in-flight
    /// state, or the notice.
    #[test]
    fn the_editor_paints_in_the_reading_pane() {
        let (mut app, _) = modal_with(vec![issue(15, "Fix login redirect")]);
        let read = screen(&mut app, 100, 30);
        assert!(read.contains("Issue #15"), "{read}");
        assert!(!read.contains("Edit issue"), "{read}");
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut Vec::new(),
        );
        let form = screen(&mut app, 100, 30);
        assert!(form.contains("Edit issue #15"), "{form}");
        assert!(form.contains("Title  Fix login redirect"), "{form}");
        assert!(form.contains(" Description "), "{form}");
        assert!(form.contains("Login bounces back to /."), "{form}");
        assert!(form.contains("#15 Fix login"), "the list stays: {form}");
        assert!(form.contains("Enter: save"), "{form}");
        let e = editor(&app).unwrap();
        assert!(
            e.title_area.width > 0 && e.body_area.height > 0,
            "rects written back"
        );
        editor_mut(&mut app).saving = true;
        assert!(screen(&mut app, 100, 30).contains("saving…"));
        let e = editor_mut(&mut app);
        e.saving = false;
        e.notice = Some("the issue needs a title".into());
        assert!(screen(&mut app, 100, 30).contains("the issue needs a title"));
    }

    /// `Ctrl+o` and a click on the reading pane's `↗ open in browser` button run
    /// one open: the footer names where the browser went either way (INPUT
    /// PARITY), and the modal stays up. The button sits pinned right on the
    /// pane's top border with its rect written back for the click, the
    /// pointer resting on it is what `hover_crumb` holds — and the editor
    /// taking the pane over takes the button with it, rect and all.
    #[test]
    fn o_and_the_browser_button_open_the_issue_the_same_way() {
        let (mut app, _project) = modal_with(vec![issue(15, "Fix login redirect")]);
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("↗ open in browser"), "{shot}");
        let button = match &app.overlay {
            Some(Overlay::Issues(v)) => v.browser_area,
            other => panic!("expected the issues modal, got {other:?}"),
        };
        assert!(button.width > 0, "the button's rect is written back");
        let at = Position::new(button.x + 1, button.y);
        assert_eq!(
            crate::ui::browser_button_under(&app, at),
            Some(HitTarget::ModalBrowser)
        );
        assert_eq!(
            crate::ui::browser_button_under(&app, Position::new(button.x - 1, button.y)),
            None
        );

        let mut out = Vec::new();
        handle_key(
            &mut app,
            key(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &mut out,
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("opened github.com/o/r/issues/15")
        );
        app.flash = None;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut out);
        assert_eq!(
            app.flash.as_deref(),
            Some("opened github.com/o/r/issues/15")
        );
        assert!(
            matches!(app.overlay, Some(Overlay::Issues(_))),
            "the modal stays up"
        );

        // `Ctrl+e`: the editor has the pane, and the button goes with it.
        handle_key(
            &mut app,
            key(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut out,
        );
        let shot = screen(&mut app, 120, 40);
        assert!(!shot.contains("open in browser"), "{shot}");
        let Some(Overlay::Issues(v)) = &app.overlay else {
            panic!("no issues modal");
        };
        assert_eq!(v.browser_area, Rect::default());
        assert_eq!(crate::ui::browser_button_under(&app, at), None);
    }
}
