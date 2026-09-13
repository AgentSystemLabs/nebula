//! The ISSUES MODAL: the selected project's open GitHub issues, listed
//! newest first down the left, the one under the cursor read on the right,
//! and two ways to put an agent on it — a QUICK PROMPT (`Enter` / `p`) or
//! one of the saved AGENT PRESETS (`e`). Either launch carries the issue as
//! an [`IssueRef`], and the create that ends it sends the issue URL to the
//! DAEMON (`ClientRequest::CreateAgent::issue_url`), which folds it into the
//! harness's context on every spawn — Claude's appended system prompt, a
//! Codex / Cursor cold spawn's first prompt — so the agent knows which
//! issue the session is for before it reads the first word of the task.
//!
//! Like the pull requests, the issues are the TUI's own business: one
//! `gh issue list` per project when the modal opens (and on `r`), one
//! `gh issue view` for the comments of the row the cursor rests on, both
//! off the loop with the answer landing on `App::issues_tx`. A `gh` that is
//! missing, unauthenticated, or pointed at a repo with no remote is an
//! ordinary "couldn't ask", said in the pane rather than flashed. Nothing
//! is written to disk: the list is a modal's worth of rows, re-asked each
//! time it opens, and kept in memory so reopening paints at once while the
//! fresh answer lands underneath.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::ProjectId;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{clamp_selection, window_start, App, Overlay};
use crate::pr_preview::{fit, wrap};
use crate::quick_prompt::{QuickLaunch, QuickReturn, QuickTarget};
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, panel_block, render_row, row_rect, truncate,
    SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
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
    /// The description, verbatim markdown, rendered as plain wrapped text
    /// like a pull request's.
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
}

/// The modal's own state. The rows live on the [`App`] (`issues`, keyed by
/// project) so a fetch that lands after the modal closed still paints the
/// next open; this holds only the cursor, the reading pane's scroll, and
/// the rects the mouse hit-tests against.
#[derive(Debug, Clone)]
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
        }
    }

    /// First visible row of the list's stateless follow-window.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    pub fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
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
/// one selected, so this works from any row; only an empty workspace has
/// nothing to list.
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
    request_list(app, project.id, project.repo_path);
    schedule_detail(app);
    app.dirty = true;
}

fn list_len(app: &App, project: &ProjectId) -> usize {
    app.issues.get(project).map_or(0, |l| l.list.len())
}

/// Ask `gh` for the project's open issues, off the loop. Skipped while
/// one is in flight — a repaint must never stack `gh` processes. A repo
/// that isn't on disk is a miss noted without spending a process. Without
/// the loop's sender installed (the unit tests) nothing is asked.
fn request_list(app: &mut App, project: ProjectId, dir: PathBuf) {
    if app.issues_inflight.contains(&project) {
        return;
    }
    if !dir.is_dir() {
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

/// Arm (or disarm) the debounced comments fetch for the row under the
/// cursor. An issue already fetched, in flight, or known unanswerable arms
/// nothing. Landing on a row rewinds the reading pane.
fn schedule_detail(app: &mut App) {
    let pending = selected_issue(app).and_then(|(issue, dir)| {
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
/// last good one, or says so when there is none. Comments replace whatever
/// the pane showed for the URL; a failed fetch is remembered so the pane
/// says so instead of spinning.
pub(crate) fn land_answer(app: &mut App, answer: IssuesAnswer) {
    match answer {
        IssuesAnswer::List { project, list } => {
            app.issues_inflight.remove(&project);
            match list {
                Some(list) => {
                    let cursor_url = match &app.overlay {
                        Some(Overlay::Issues(view)) if view.project == project => app
                            .issues
                            .get(&project)
                            .and_then(|l| l.list.get(view.selected))
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
    }
    app.dirty = true;
}

/// `r` in the modal: ask for the list again now, and the selected issue's
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
    let issue = app.issues.get(&view.project)?.list.get(view.selected)?;
    Some((issue.clone(), view.dir.clone()))
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

// ---- launching ----

/// Where a launch from the modal lands: the selected WORKTREE when it is
/// one of this project's real checkouts (not a stand-in git is still
/// cutting, not an OPEN PRS row), else the project's ROOT WORKTREE — which
/// every project has whether the panel shows it or not. `Ctrl+N` in the box
/// flips to a fresh worktree named after the issue.
fn launch_target(app: &App, project: &ProjectId) -> Option<QuickTarget> {
    if let Some(w) = app.selected_worktree() {
        if &w.project_id == project && !app.is_placeholder_worktree(&w.id) {
            return Some(QuickTarget::Worktree(w.id.clone()));
        }
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
    let Some(target) = launch_target(app, &project) else {
        app.flash = Some("issues: the project has no worktree to launch into".into());
        return None;
    };
    Some(
        QuickLaunch::from_config(target, &crate::config::Config::load())
            .with_issue(Some(issue.launch_ref())),
    )
}

/// `Enter` / `p`: the QUICK PROMPT for the issue. The box replaces the
/// modal; Esc from it lands on the panels, `i` reopens the list.
fn open_prompt_for_selected(app: &mut App) {
    if let Some(launch) = launch_for_selected(app) {
        crate::event_loop::open_prompt(app, crate::app::PromptKind::QuickPrompt(launch));
    }
}

/// `e`: pick one of the saved AGENT PRESETS for the issue. The picker hands
/// its pick to the same box `Enter` opens, with the preset applied; with no
/// presets saved the modal stays up and the footer says where to add one.
fn open_preset_for_selected(app: &mut App) {
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_preset_picker(
            app,
            QuickReturn {
                launch,
                text: String::new(),
            },
        );
    }
}

// ---- keys and mouse ----

/// Keys in the ISSUES MODAL.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let half = (view.view_height / 2).max(1) as i32;
    let page = view.view_height.max(1) as i32;
    let selected = view.selected as i64;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => app.overlay = None,
        KeyCode::Char('j') | KeyCode::Down if !shift => select(app, selected + 1),
        KeyCode::Char('k') | KeyCode::Up if !shift => select(app, selected - 1),
        // The reading pane scrolls on the diff modal's keys.
        KeyCode::Char('d') if ctrl => view.scroll_by(half),
        KeyCode::Char('u') if ctrl => view.scroll_by(-half),
        // Shift+j/k scroll the pane a line: `J` in most terminals, a
        // shifted `j` under the kitty protocol.
        KeyCode::Down | KeyCode::Char('j') if shift => view.scroll_by(1),
        KeyCode::Up | KeyCode::Char('k') if shift => view.scroll_by(-1),
        KeyCode::Char('J') => view.scroll_by(1),
        KeyCode::Char('K') => view.scroll_by(-1),
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        KeyCode::Enter | KeyCode::Char('p') => open_prompt_for_selected(app),
        KeyCode::Char('e') => open_preset_for_selected(app),
        KeyCode::Char('o') => {
            if let Some((issue, _)) = selected_issue(app) {
                if !crate::event_loop::open_url(&issue.url) {
                    app.flash = Some("could not open the browser".into());
                }
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => refresh(app),
        _ => {}
    }
    app.dirty = true;
}

/// Mouse in the ISSUES MODAL: the wheel moves the cursor over the list and
/// scrolls the reading pane over it, a click on a row selects it (a launch
/// is `Enter`, not a click — the row is something to read first), and a
/// click outside closes (`overlay_close`); everything else is swallowed.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, mouse_pos: Position) {
    let Some(Overlay::Issues(view)) = &mut app.overlay else {
        return;
    };
    let over_body = view.body_area.contains(mouse_pos);
    let selected = view.selected as i64;
    match mouse.kind {
        MouseEventKind::ScrollUp if over_body => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::ScrollDown if over_body => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => select(app, selected - 1),
        MouseEventKind::ScrollDown => select(app, selected + 1),
        MouseEventKind::Down(MouseButton::Left) => {
            let list = view.list_area;
            if list.contains(mouse_pos) {
                let start = view.window_start(list.height as usize);
                let index = start + (mouse.row - list.y) as usize;
                select(app, index as i64);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

// ---- drawing ----

/// The reading pane as styled lines: headline, state row, description,
/// then the conversation once it has landed.
pub fn lines(
    issue: &Issue,
    detail: Option<&IssueDetail>,
    comments_failed: bool,
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
        for row in wrap(issue.body.trim_end(), body_w) {
            out.push(Line::from(Span::styled(format!("{INDENT}{row}"), muted)));
        }
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
                for row in wrap(c.body.trim_end(), body_w.saturating_sub(2)) {
                    out.push(Line::from(Span::styled(format!("{INDENT}  {row}"), muted)));
                }
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
    out
}

/// The ISSUES MODAL: the list down the left, the reading pane on the right.
pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &IssuesView, th: Theme) {
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
    let selected = view.selected.min(rows.len().saturating_sub(1));

    // ---- left: the list ----
    let title = format!(
        "Issues — {} ({}{})",
        view.project_name,
        rows.len(),
        if inflight { ", refreshing…" } else { "" }
    );
    let block = panel_block(&title, true, th).title_bottom(
        Line::from(Span::styled(
            " Enter/p: prompt  e: preset  o: browser  r: refresh ",
            Style::default().fg(th.dim),
        ))
        .left_aligned(),
    );
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    if rows.is_empty() {
        let text = if failed {
            "couldn't list issues — is gh installed and logged in?"
        } else if inflight || app.issues_tx.is_some() && !app.issues.contains_key(&view.project) {
            "asking GitHub…"
        } else {
            "no open issues"
        };
        empty_list_row(f, list_inner, text, th);
    }
    let start = view.window_start(list_inner.height as usize);
    for (i, issue) in rows.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(list_inner, i - start) else {
            break;
        };
        let budget = (list_inner.width as usize).saturating_sub(2);
        // `#15 title` left, the day it was opened pinned right, dim.
        let opened = day(&issue.created_at).to_string();
        let opened_w = opened.chars().count();
        let text_budget = budget.saturating_sub(if opened_w > 0 { opened_w + 2 } else { 0 });
        let label = truncate(&issue.label(), text_budget);
        let used = label.chars().count();
        let mut spans = vec![
            Span::styled(format!("#{} ", issue.number), Style::default().fg(th.dim)),
            Span::raw(
                label
                    .strip_prefix(&format!("#{} ", issue.number))
                    .unwrap_or(&label)
                    .to_string(),
            ),
        ];
        if opened_w > 0 && used + opened_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - opened_w)));
            spans.push(Span::styled(opened, Style::default().fg(th.dim)));
        }
        render_row(f, row_area, spans, i == selected, true, th);
    }

    // ---- right: the reading pane ----
    let current = rows.get(selected);
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
    let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
    f.render_widget(Paragraph::new(shown), body_inner);

    // Write-back (draw works on a clone): the rects the mouse hit-tests,
    // the pane's size for paging, and the clamped cursor and scroll.
    if let Some(Overlay::Issues(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = list_inner;
        v.body_area = body_inner;
        v.view_height = body_inner.height;
        v.body_lines = lines.len();
        v.selected = selected;
        v.scroll = scroll;
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
        let waiting = text(&lines(&i, None, false, 60, Theme::default()));
        assert!(waiting.starts_with(" #15 Fix login redirect"), "{waiting}");
        assert!(
            waiting.contains("open · webdevcody · 2026-09-10"),
            "{waiting}"
        );
        assert!(waiting.contains("bug · ui"), "{waiting}");
        assert!(waiting.contains("Login bounces back to /."), "{waiting}");
        assert!(waiting.contains("reading the comments…"), "{waiting}");

        let failed = text(&lines(&i, None, true, 60, Theme::default()));
        assert!(failed.contains("couldn't read the comments"), "{failed}");

        let detail = IssueDetail {
            url: i.url.clone(),
            comments: vec![IssueComment {
                author: "kate".into(),
                at: "2026-09-11T08:00:00Z".into(),
                body: "same here".into(),
            }],
        };
        let read = text(&lines(&i, Some(&detail), false, 60, Theme::default()));
        assert!(read.contains("── 1 comment ──"), "{read}");
        assert!(read.contains("kate · 2026-09-11"), "{read}");
        assert!(read.contains("same here"), "{read}");
        let quiet = IssueDetail {
            url: i.url.clone(),
            comments: vec![],
        };
        let none = text(&lines(&i, Some(&quiet), false, 60, Theme::default()));
        assert!(none.contains("── no comments ──"), "{none}");
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
            for line in lines(&i, Some(&detail), false, w, Theme::default()) {
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
}
