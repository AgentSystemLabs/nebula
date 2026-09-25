//! The PULL REQUESTS MODAL: the selected project's open pull requests,
//! listed down the left in the PROJECT OPEN PRS GROUP's order — newest
//! first, the drafts sunk below the finished ones — the one under the
//! cursor read on the right, and the ISSUES MODAL's ways to put an agent
//! on it: a QUICK PROMPT (`Enter`), one of the saved AGENT PRESETS
//! (`Shift+Tab`), or a bare harness pick (`Tab`, the group row's NEW
//! SESSION PICKER) — the QUICK PROMPT box's own three keys. Every one of
//! them is a PR SESSION, launched exactly as the group's row launches it
//! (`quick_prompt::pr_launch_for`): the create is a `CreatePrAgent`, the
//! DAEMON runs the session in the project's checkout of the pull
//! request's head branch — reused when one is there, cut otherwise, its
//! stand-in rows up under the pull request from the moment Enter is
//! pressed — and the PR's URL rides the harness's context.
//!
//! The list's filter is live from the moment the modal opens, as the
//! DIFF VIEWER's and the FILE FINDER's are: every letter typed narrows
//! the rows to the fuzzy matches of `#42 title` (`fuzzy::rank`), best
//! first, the cursor on the best, and Esc clears it before a second Esc
//! closes. So the verbs are chords: `Ctrl+c` or `Ctrl+y` leaves a comment (the
//! COMMENT BOX the group row's `y` opens, which comes back to the modal
//! on its row), `Ctrl+g` reads the whole diff, `Ctrl+o` opens the pull
//! request in the browser, `Ctrl+r` asks GitHub again.
//!
//! Nothing is fetched here the panels do not already keep. The rows are
//! the project's open list (`App::open_prs`) — kept warm on the OPEN PRS
//! beat and remembered across launches (`pr_cache`) — so the modal paints
//! at once, and opening it on a list older than [`FRESH`] asks again
//! underneath. The reading pane is the PR PREVIEW's (`pr_preview::lines`),
//! its body and conversation fetched on the pane's debounce into the same
//! `App::pr_detail`, so a pull request read in one is read in the other.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::{ClientRequest, ProjectId};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{
    clamp_selection, window_start, App, HitTarget, Overlay, PendingPrDetail, PromptKind,
};
use crate::pr_preview::fit;
use crate::pull_request::{OpenPr, PrDetail};
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, fuzzy_highlight_styled, panel_block, render_row, row_rect,
    search_line, truncate, visible_positions, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

/// A list younger than this is what opening the modal shows, with no
/// second ask — the ISSUES MODAL's window, for the same reason: the list
/// the OPEN PRS beat landed moments ago *is* the answer. `r` asks
/// regardless.
pub(crate) const FRESH: std::time::Duration = crate::issues::FRESH;
/// How long the cursor rests on a row before its body and conversation
/// are fetched — the pane's own debounce, so walking the list with `j`
/// fetches only the rows actually paused on.
pub(crate) const DETAIL_DEBOUNCE: std::time::Duration = crate::event_loop::PR_DETAIL_DEBOUNCE;
/// Left inset of the reading pane's own lines — the PR PREVIEW's.
const INDENT: &str = crate::pr_preview::INDENT;
/// The list column's share of the modal, and its floor. The ISSUES MODAL
/// is laid out the same.
pub(crate) const LIST_PCT: u16 = 38;
pub(crate) const MIN_LIST_W: u16 = 24;
/// Lines one wheel notch scrolls the reading pane.
pub(crate) const WHEEL_LINES: i32 = 3;

/// The modal's own state. The rows live on the [`App`] (`open_prs`, keyed
/// by project), where the panels read them too; this holds only the
/// cursor, the reading pane's scroll, and the rects the mouse hit-tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestsView {
    pub project: ProjectId,
    /// The project's row name, for the list's title.
    pub project_name: String,
    /// The checkout `gh` runs from.
    pub dir: PathBuf,
    /// Cursor into the project's list.
    pub selected: usize,
    /// The pull request under the cursor, by URL: a refresh that reorders
    /// the list (a draft marked ready rises above the drafts) keeps the
    /// cursor on it, and one that retired it lands on its neighbour
    /// ([`list_changed`]).
    pub selected_url: Option<String>,
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
    /// `#42 title`, best first ([`visible_rows`]), as every letter lands.
    /// Empty shows every row in the list's order.
    pub query: TextInput,
    /// Where the cursor sat among the visible rows as of the last draw:
    /// the follow-window's anchor, and what a click's row math counts
    /// from.
    pub cursor_row: usize,
    /// The last click on a row — when, and which pull request — so a
    /// second click on the same row inside the DOUBLE-CLICK window opens it
    /// in the browser (`event_loop::is_double_click`).
    pub last_row_click: Option<(std::time::Instant, u64)>,
}

impl PullRequestsView {
    pub fn new(project: ProjectId, project_name: String, dir: PathBuf) -> Self {
        Self {
            project,
            project_name,
            dir,
            selected: 0,
            selected_url: None,
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            browser_area: Rect::default(),
            query: TextInput::new(),
            cursor_row: 0,
            last_row_click: None,
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

// ---- opening, fetching, following ----

/// The hotkey: the PULL REQUESTS MODAL for the selected PROJECT. Every
/// panel has one selected, so this works from any row; only a machine
/// with no project has nothing to list. The cursor starts on the pull request
/// the Worktrees cursor rests on, when it rests on one — the row the user
/// was already reading.
pub(crate) fn open(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        app.flash = Some("pull requests: select a project first".into());
        return;
    };
    let mut view = PullRequestsView::new(
        project.id.clone(),
        project.name.clone(),
        project.repo_path.clone(),
    );
    let list = rows(app, &project.id);
    let start = app
        .selected_worktree_pr()
        .and_then(|pr| list.iter().position(|row| row.url == pr.url))
        .unwrap_or(0);
    view.selected = clamp_selection(start as i64, list.len());
    view.selected_url = list.get(view.selected).map(|pr| pr.url.clone());
    app.overlay = Some(Overlay::PullRequests(view));
    // A list the beat landed moments ago is the answer; an older one
    // paints now while a fresh copy lands underneath.
    if !is_fresh(app, &project.id) {
        request_list(app, &project.id);
    }
    schedule_detail(app);
    app.dirty = true;
}

/// Close the modal. The pane behind reads the Worktrees cursor's pull
/// request again, and the modal's cursor may have taken over the fetch it
/// was waiting on — so that one is armed again, without touching the
/// pane's scroll.
fn close(app: &mut App) {
    app.overlay = None;
    let pending = app.previewed_pr().and_then(|pr| {
        let dir = app.selected_project()?.repo_path.clone();
        pending_for(app, pr.url, pr.number, dir)
    });
    app.pending_pr_detail = pending.map(|p| (p, std::time::Instant::now() + DETAIL_DEBOUNCE));
}

/// Put the modal back as it was — the COMMENT BOX stood in for it — on the
/// same pull request, followed by URL in case the list moved underneath.
pub(crate) fn reopen(app: &mut App, view: PullRequestsView) {
    app.overlay = Some(Overlay::PullRequests(view));
    list_changed(app);
    schedule_detail(app);
    app.dirty = true;
}

/// The project's open pull requests, as the group shows them (drafts and
/// all: the modal lists everything open, whatever `hide_draft_prs` keeps
/// out of the panel).
fn rows<'a>(app: &'a App, project: &ProjectId) -> &'a [OpenPr] {
    app.open_prs
        .get(project)
        .map_or(&[], |open| open.list.as_slice())
}

/// Is there a filter to apply — text in the row beyond whitespace?
fn has_query(view: &PullRequestsView) -> bool {
    view.query.split_whitespace().next().is_some()
}

/// The rows the filter leaves, top to bottom: indices into `list`, each
/// with the matched char positions of its `#42 title` (lit when drawn);
/// every row in list order with nothing typed. Worked out afresh on every
/// call rather than kept — a project's open pull requests are a handful
/// — so it can never go stale against the list.
fn visible_rows(query: &str, list: &[OpenPr]) -> Vec<(usize, Vec<usize>)> {
    let labels: Vec<String> = list.iter().map(|pr| pr.label()).collect();
    crate::fuzzy::rank(query, labels.iter().map(String::as_str))
}

/// The row under the cursor, as an index into `list`: `selected` while
/// the filter shows it, else the filter's best match — a refresh may have
/// moved the cursor's pull request under a row the filter hides — and
/// `selected` clamped onto the list with nothing typed. None with no row
/// to be on: an empty list, or a filter nothing matches.
fn cursor_index(view: &PullRequestsView, list: &[OpenPr]) -> Option<usize> {
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

/// A list that landed within [`FRESH`]: the modal opens on it as it is.
fn is_fresh(app: &App, project: &ProjectId) -> bool {
    app.open_prs
        .get(project)
        .is_some_and(|open| open.at.elapsed() < FRESH)
}

/// Ask for the project's open list on the loop's next turn, past its
/// beat — `Shift+R`'s path (`App::pr_refresh_requested`), which asks for
/// the selected project, the modal's. A lookup already in flight is left
/// to land.
fn request_list(app: &mut App, project: &ProjectId) {
    if let Some(open) = app.open_prs.get_mut(project) {
        open.due = std::time::Instant::now();
    }
    app.pr_refresh_requested = true;
}

/// The pull request under the cursor, while the modal is up and the list
/// has a row the filter shows.
fn selected_pr(app: &App) -> Option<OpenPr> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let list = rows(app, &view.project);
    cursor_index(view, list).and_then(|i| list.get(i).cloned())
}

/// The URL of the pull request under the cursor, for the browser.
fn selected_url(app: &App) -> Option<String> {
    selected_pr(app).map(|pr| pr.url)
}

/// The detail fetch a pull request is owed, if any: none for one already
/// read, in flight, or known unanswerable — except a body the cache
/// hydrated (`pr_detail_stale`), which shows at once and is fetched fresh
/// over the top, as the pane's is.
fn pending_for(app: &App, url: String, number: u64, dir: PathBuf) -> Option<PendingPrDetail> {
    let fresh = app.pr_detail.contains_key(&url) && !app.pr_detail_stale.contains(&url);
    if fresh || app.pr_detail_inflight.contains(&url) || app.pr_detail_failed.contains(&url) {
        return None;
    }
    Some(PendingPrDetail { url, number, dir })
}

/// Arm (or disarm) the debounced fetch of the row under the modal's
/// cursor, on the slot the pane's own fetch uses (`App::pending_pr_detail`)
/// — the loop fires it and lands the answer in `App::pr_detail` either way.
/// While the modal is up this is the one that decides what that slot holds
/// (`event_loop::schedule_pr_detail` defers to it).
pub(crate) fn schedule_detail(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let dir = view.dir.clone();
    let pending = selected_pr(app).and_then(|pr| pending_for(app, pr.url, pr.number, dir));
    app.pending_pr_detail = pending.map(|p| (p, std::time::Instant::now() + DETAIL_DEBOUNCE));
}

/// The list under the modal changed — an answer landed, or a detail said
/// a pull request merged and retired its row. The cursor stays on its
/// pull request wherever the new list put it; one that is gone leaves the
/// cursor on the row that took its index, with that row's body asked for
/// and the pane rewound. Run from wherever `App::open_prs` changes.
pub(crate) fn list_changed(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let found = view
        .selected_url
        .as_ref()
        .and_then(|url| list.iter().position(|pr| &pr.url == url));
    let (index, url) = match found {
        Some(i) => (i, view.selected_url.clone()),
        None => {
            let i = clamp_selection(view.selected as i64, list.len());
            (i, list.get(i).map(|pr| pr.url.clone()))
        }
    };
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let moved = url != view.selected_url;
    view.selected = index;
    view.selected_url = url;
    if moved {
        view.scroll = 0;
        schedule_detail(app);
    }
    app.dirty = true;
}

/// Move the cursor to `index` (clamped): the pane rewinds and the row's
/// body is asked for once the cursor rests.
fn select(app: &mut App, index: i64) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let next = clamp_selection(index, list.len());
    let url = list.get(next).map(|pr| pr.url.clone());
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    if next != view.selected || url != view.selected_url {
        view.selected = next;
        view.selected_url = url;
        view.scroll = 0;
    }
    schedule_detail(app);
    app.dirty = true;
}

/// ↑/↓, the wheel: the cursor `delta` rows through the visible ones —
/// the filter's matches while one is typed — clamped at either end.
fn step(app: &mut App, delta: i64) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
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
/// pane rewinds onto it and its body is asked for, as any move does — or
/// stays where it is once nothing is typed, so the row just found keeps
/// the cursor after Esc has cleared the letters that found it. A filter
/// nothing matches moves nothing: the list says so, the pane has no row
/// to read, and the next letter or Backspace decides.
fn query_changed(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
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
    if let Some(Overlay::PullRequests(view)) = &mut app.overlay {
        view.query.clear();
    }
    query_changed(app);
}

/// A bracketed paste lands in the filter, as one line, and narrows the
/// rows as typing it would. True whenever the modal is up: the filter is
/// always live.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return false;
    };
    view.query.insert_str(text);
    query_changed(app);
    true
}

/// `Ctrl+r`: ask for the list again now, and the selected pull request's body
/// over the cached copy. The rows stay until the answer lands.
fn refresh(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    request_list(app, &project);
    if let Some(pr) = selected_pr(app) {
        if !app.pr_detail_inflight.contains(&pr.url) {
            app.pr_detail_failed.remove(&pr.url);
            app.pending_pr_detail = Some((
                PendingPrDetail {
                    url: pr.url,
                    number: pr.number,
                    dir,
                },
                std::time::Instant::now(),
            ));
        }
    }
    app.flash = Some("refreshing pull requests…".into());
    app.dirty = true;
}

// ---- launching ----

/// The launch the row under the cursor describes — the group row's, for
/// this pull request: the `quick_prompt_kind` SETTING's harness, the pull
/// request carried as `QuickLaunch::pr`, addressed to the project's root.
fn launch_for_selected(app: &mut App) -> Option<QuickLaunch> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let project = view.project.clone();
    let Some(pr) = selected_pr(app) else {
        app.flash = Some("no pull request selected".into());
        return None;
    };
    crate::quick_prompt::pr_launch_for(app, &project, &pr)
}

/// `Enter`: the QUICK PROMPT for a PR SESSION on the pull request.
/// The box goes up over the modal, which stays on screen under it: Esc
/// puts the modal back on the row (`QuickLaunch::under`), and the launch
/// closes it onto the new session's card.
fn open_prompt_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_pr_box(app, launch.with_under(under));
    }
}

/// `Shift+Tab`: one of the saved AGENT PRESETS as a PR SESSION on the pull
/// request. The picker goes up over the modal, as `Enter`'s box does, and
/// hands its pick to that same box with the preset applied; Esc puts the
/// modal back on the row.
fn open_preset_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_preset_picker(app, QuickReturn::fresh(launch.with_under(under)));
    }
}

/// `Tab`: the NEW SESSION PICKER's harness rows for a PR SESSION on the pull
/// request — `n` on the group's row — launching bare on Enter, or through
/// the MODEL / EFFORT submenus on `→`.
fn open_harness_picker_for_selected(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let project = view.project.clone();
    let Some(pr) = selected_pr(app) else {
        app.flash = Some("no pull request selected".into());
        return;
    };
    // The PROJECT's ROOT WORKTREE: what a PR SESSION create is addressed to.
    let Some(root) = app.root_worktree(&project) else {
        app.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
        return;
    };
    crate::agent_picker::open_kind_picker(
        app,
        crate::agent_picker::KindPicker::pr_session(root, &pr),
    );
}

/// `Ctrl+c`: the COMMENT BOX for the pull request under the cursor, carrying the
/// modal so Enter and Esc come back to it on the row. A draft a refused
/// post left for this pull request fills the box.
fn open_comment_for_selected(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let view = view.clone();
    let Some(pr) = selected_pr(app) else {
        app.flash = Some("no pull request selected".into());
        return;
    };
    let draft = app.pr_comment_drafts.remove(&pr.url).unwrap_or_default();
    crate::event_loop::reopen_prompt_with(
        app,
        PromptKind::PrComment {
            number: pr.number,
            label: pr.label(),
            url: pr.url,
            back: Some(Box::new(view)),
        },
        draft,
    );
}

/// `Ctrl+o`, and a click on the reading pane's `↗ open in browser` button
/// (`HitTarget::ModalBrowser`): the pull request under the cursor in the
/// browser, through the very `event_loop::open_link` a card's `⇧V` and
/// `⇧I` run — the footer says where it went, or that it could not — and the pull request is marked
/// read on the way out, its conversation about to be on screen.
/// Nothing under the cursor opens nothing. INPUT PARITY: the key and the
/// click end in the same state.
pub(crate) fn open_in_browser(app: &mut App, out: &mut Vec<ClientRequest>) {
    if let Some(url) = selected_url(app) {
        crate::event_loop::open_link(app, &url, out);
    }
}

// ---- keys and mouse ----

/// Keys in the PULL REQUESTS MODAL. The filter is always live, so
/// letters type — the modal's own hotkey and `q` among them — and the
/// verbs are chords; only Esc closes, once the filter is clear.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
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
        KeyCode::Esc => close(app),
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
        // Tab picks a harness, Shift+Tab a preset (a shifted Tab under the
        // kitty protocol is the same key).
        KeyCode::Enter => open_prompt_for_selected(app),
        KeyCode::BackTab => open_preset_for_selected(app),
        KeyCode::Tab if shift => open_preset_for_selected(app),
        KeyCode::Tab => open_harness_picker_for_selected(app),
        // `Ctrl+y` is the grid's `y` (reply) as a chord, the letters being
        // the filter's.
        KeyCode::Char('c') | KeyCode::Char('y') if ctrl => open_comment_for_selected(app),
        KeyCode::Char('g') if ctrl => {
            if let Some(pr) = selected_pr(app) {
                crate::event_loop::request_pr_diff_for(app, pr.number, pr.url.clone(), pr.label());
            }
        }
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

/// Mouse in the PULL REQUESTS MODAL: the wheel moves the cursor over the
/// rows the filter leaves and scrolls the reading pane over it, a click on
/// a row selects it (a launch is `Enter`, not a click — the row is
/// something to read first), a double-click on a row opens that pull
/// request in the browser — the very open `Ctrl+o` and the `↗ open in
/// browser` button run — and a click outside closes (`overlay_close`);
/// everything else is swallowed.
pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
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
            let prs: &[OpenPr] = app
                .open_prs
                .get(&view.project)
                .map_or(&[], |open| open.list.as_slice());
            let visible = visible_rows(&view.query, prs);
            if let Some(row) = crate::list_hit::row_at(list, first, visible.len(), mouse_pos) {
                let index = visible[row].0;
                let double =
                    crate::event_loop::is_double_click(&mut view.last_row_click, prs[index].number);
                select(app, index as i64);
                if double {
                    open_in_browser(app, out);
                }
            }
        }
        _ => {}
    }
    app.dirty = true;
}

/// The footer's key line for the modal.
pub(crate) fn footer_hint() -> &'static str {
    "type to filter  ↑/↓ ^n/^p: pull request  PgUp/PgDn ^d/^u: read  Enter: prompt an agent  Tab: harness  ⇧Tab: preset  ^c/^y: comment  ^g: diff  ^o: browser  ^r: refresh  Esc: clear / close"
}

// ---- drawing ----

/// The reading pane as styled lines: the PR PREVIEW once the body has
/// landed, the row's own headline and a word on the fetch until then —
/// and, while a comment of yours is on its way, a line saying so.
pub fn lines(
    pr: &OpenPr,
    detail: Option<&PrDetail>,
    failed: bool,
    posting: bool,
    width: usize,
    th: Theme,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let mut out = match detail {
        Some(detail) => crate::pr_preview::lines(detail, width, th),
        None => {
            let mut out = vec![
                fit(
                    vec![
                        Span::styled(format!("{INDENT}#{} ", pr.number), dim),
                        Span::styled(
                            pr.title.clone(),
                            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                        ),
                    ],
                    width,
                ),
                Line::from(""),
            ];
            let message = if failed {
                "couldn't read this pull request — is gh installed and logged in? o still opens it in the browser."
            } else {
                "reading it…"
            };
            let wrap_w = width.saturating_sub(INDENT.len() + 1).max(20);
            out.extend(
                crate::pr_preview::wrap(message, wrap_w)
                    .into_iter()
                    .map(|l| Line::from(Span::styled(format!("{INDENT}{l}"), dim))),
            );
            out
        }
    };
    if posting {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!("{INDENT}── posting your comment… ──"),
            dim,
        )));
    }
    out
}

/// One list row's spans: `#42` dim, the title in the group row's color
/// (`pr_row::look` — dimmed for a draft, red for a pull request GitHub
/// says cannot merge), and the badge pinned right — the trouble's word,
/// else `draft` — so a row reads the way its group row does. The chars
/// the filter matched (`positions`, into the row's `#42 title`) are lit.
fn row_spans(pr: &OpenPr, positions: &[usize], budget: usize, th: Theme) -> Vec<Span<'static>> {
    let trouble = pr.trouble();
    let look = crate::pr_row::look(pr.standing(), trouble, th);
    let badge = match trouble {
        Some(trouble) => Some(trouble.badge()),
        None => pr.is_draft.then(|| pr.badge()),
    };
    let badge_w = badge.map_or(0, |b| b.chars().count());
    let text_budget = budget.saturating_sub(if badge_w > 0 { badge_w + 2 } else { 0 });
    let full = pr.label();
    let label = truncate(&full, text_budget);
    let positions = visible_positions(positions, &label, &full);
    let number = format!("#{} ", pr.number);
    // A row with no title is its number alone.
    let title = label.strip_prefix(&number).unwrap_or_default().to_string();
    let number_w = number.chars().count();
    let used = number_w + title.chars().count();
    let number_color = if trouble.is_some() {
        look.label
    } else {
        th.dim
    };
    // The positions split where the number ends: the title's own count
    // from its first char.
    let split = positions.partition_point(|&p| p < number_w);
    let title_positions: Vec<usize> = positions[split..].iter().map(|p| p - number_w).collect();
    let mut spans = fuzzy_highlight_styled(
        &number,
        &positions[..split],
        Style::default().fg(number_color),
        th,
    );
    spans.extend(fuzzy_highlight_styled(
        &title,
        &title_positions,
        Style::default().fg(look.label),
        th,
    ));
    if let Some(badge) = badge {
        if used + badge_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - badge_w)));
            spans.push(Span::styled(badge, Style::default().fg(look.badge)));
        }
    }
    spans
}

/// The PULL REQUESTS MODAL: the list down the left, the reading pane on
/// the right. `backdrop` draws it as the layer under a QUICK PROMPT box
/// opened from it (`QuickLaunch::under`): dim frames and an unfocused
/// cursor row, the box in front having the eye.
pub(crate) fn draw(
    f: &mut Frame,
    app: &mut App,
    view: &PullRequestsView,
    th: Theme,
    backdrop: bool,
) {
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

    let rows: Vec<OpenPr> = rows(app, &view.project).to_vec();
    let inflight = app.open_prs_inflight.contains(&view.project);
    let asked = app.open_prs.contains_key(&view.project);
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
        "Pull requests — {} ({}{})",
        view.project_name,
        count,
        if inflight { ", refreshing…" } else { "" }
    );
    let block = panel_block(&title, !backdrop, th).title_bottom(
        Line::from(Span::styled(
            // The launches; the footer spells out the rest.
            " Enter: prompt  Tab: harness  ⇧Tab: preset ",
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
    let rows_area = crate::ui::below_first_row(list_inner);
    if rows.is_empty() {
        let text = if inflight || !asked {
            "asking GitHub…"
        } else {
            "no open pull requests"
        };
        empty_list_row(f, rows_area, text, th);
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no pull requests match", th);
    }
    let start = window_start(cursor_row, rows_area.height as usize);
    let budget = (rows_area.width as usize).saturating_sub(2);
    for (row, (index, positions)) in visible.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(rows_area, row - start) else {
            break;
        };
        render_row(
            f,
            row_area,
            row_spans(&rows[*index], positions, budget, th),
            Some(*index) == cursor,
            !backdrop,
            th,
        );
    }

    // ---- right: the reading pane ----
    let current = cursor.and_then(|i| rows.get(i));
    // The frame names the number; the headline inside carries the title.
    let body_title = match current {
        Some(pr) => format!("Pull request #{}", pr.number),
        None => "Pull request".to_string(),
    };
    let lines: Vec<Line> = match current {
        Some(pr) => lines(
            pr,
            app.pr_detail.get(&pr.url),
            app.pr_detail_failed.contains(&pr.url),
            app.pr_comment_inflight.contains(&pr.url),
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
    if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = rows_area;
        v.cursor_row = cursor_row;
        v.body_area = body_inner;
        v.browser_area = browser_area;
        v.view_height = body_inner.height;
        v.body_lines = lines.len();
        // A cursor the filter had to move (see `cursor_index`) is settled
        // onto its row, URL and all, so a refresh follows that one.
        if let Some(index) = cursor {
            if index != v.selected {
                v.selected = index;
                v.selected_url = rows.get(index).map(|pr| pr.url.clone());
            }
        }
        v.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{Checks, Health, PrLaunch};
    use crate::quick_prompt::QuickTarget;
    use nebula_core::WorktreeId;

    const DIR: &str = "/nonexistent/nebula-pr-modal";

    fn pr(number: u64, title: &str, is_draft: bool) -> OpenPr {
        OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft,
            health: Health::default(),
            head: format!("branch-{number}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn shifted(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// An app with one project (`demo`), its ROOT WORKTREE when `root`,
    /// and `list` as the project's open pull requests, landed just now.
    fn app_with(list: Vec<OpenPr>, root: bool) -> (App, ProjectId) {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.tree.projects.push(nebula_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: DIR.into(),
            sort_order: 0,
        });
        if root {
            app.tree.worktrees.push(nebula_core::Worktree {
                id: WorktreeId("w-root".into()),
                project_id: project.clone(),
                path: DIR.into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            });
        }
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project.clone(),
            crate::app::OpenPrs {
                list,
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        (app, project)
    }

    /// Config and presets pinned to temp files, so a launch resolves its
    /// harness off neither of the dev's own.
    fn pinned(f: impl FnOnce()) {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            crate::agent_presets::with_presets_path(dir.path().join("presets.json"), f)
        });
    }

    fn view(app: &App) -> &PullRequestsView {
        match &app.overlay {
            Some(Overlay::PullRequests(v)) => v,
            other => panic!("expected the pull requests modal, got {other:?}"),
        }
    }

    fn pending_url(app: &App) -> Option<&str> {
        app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str())
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::PullRequests(v)) = app.overlay.clone() else {
                panic!("no pull requests modal");
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

    /// The hotkey opens on the project's open list as it is — no second
    /// ask for a list that just landed — with the first row's body asked
    /// for on the debounce; an older list is asked for again underneath.
    #[test]
    fn opening_reads_the_first_pull_request_and_asks_only_for_a_stale_list() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        open(&mut app);
        assert_eq!(view(&app).project, project);
        assert_eq!(view(&app).selected, 0);
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/42"));
        assert!(!app.pr_refresh_requested, "fresh: nothing to ask");

        app.overlay = None;
        let stale = std::time::Instant::now()
            .checked_sub(FRESH + std::time::Duration::from_secs(1))
            .expect("machine up for a minute");
        app.open_prs.get_mut(&project).unwrap().at = stale;
        open(&mut app);
        assert!(app.pr_refresh_requested, "stale: asked on the next turn");
        assert!(app.open_prs_lookup_due(&project), "past its beat");
    }

    /// A body already read arms nothing; one the cache hydrated is shown
    /// and fetched fresh over the top, as the pane's is.
    #[test]
    fn a_read_body_arms_nothing_and_a_hydrated_one_is_fetched_again() {
        let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
        let url = "https://github.com/o/r/pull/42".to_string();
        app.pr_detail.insert(url.clone(), detail(42, "Fix login"));
        open(&mut app);
        assert_eq!(pending_url(&app), None, "read: nothing to ask");
        app.overlay = None;
        app.pr_detail_stale.insert(url.clone());
        open(&mut app);
        assert_eq!(pending_url(&app), Some(url.as_str()));
    }

    /// ↑/↓ (and Ctrl+n/p) walk the rows, each arming its own fetch; only
    /// Esc closes — the hotkey and `q` type into the filter, as every
    /// letter does — and a typed filter takes the first Esc.
    #[test]
    fn arrows_walk_the_rows_and_only_esc_closes() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(view(&app).selected, 1);
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/41"));
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(view(&app).selected, 1, "clamped at the last row");
        handle_key(&mut app, key(KeyCode::Up), &mut Vec::new());
        assert_eq!(view(&app).selected, 0);
        handle_key(&mut app, ctrl('n'), &mut Vec::new());
        assert_eq!(view(&app).selected, 1);
        handle_key(&mut app, ctrl('p'), &mut Vec::new());
        assert_eq!(view(&app).selected, 0);
        for letter in ['q', 'v'] {
            handle_key(&mut app, key(KeyCode::Char(letter)), &mut Vec::new());
            assert!(
                matches!(&app.overlay, Some(Overlay::PullRequests(_))),
                "{letter} types rather than closing"
            );
        }
        assert_eq!(view(&app).query.as_str(), "qv");
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(
            matches!(&app.overlay, Some(Overlay::PullRequests(_))),
            "the first Esc only clears the filter"
        );
        assert!(view(&app).query.is_empty());
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(app.overlay.is_none(), "the second closes");
    }

    /// A refresh that reorders the list keeps the cursor on its pull
    /// request; one that retired it leaves the cursor on the row that took
    /// its place, reading that one from the top.
    #[test]
    fn the_cursor_follows_its_pull_request_across_a_new_list() {
        let (mut app, project) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        app.pending_pr_detail = None;
        app.open_prs.get_mut(&project).unwrap().list = vec![
            pr(43, "New", false),
            pr(42, "Fix login", false),
            pr(41, "Spike", true),
        ];
        list_changed(&mut app);
        assert_eq!(view(&app).selected, 2, "still on #41");
        assert_eq!(pending_url(&app), None, "same row: nothing re-armed");

        if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
            v.scroll = 5;
        }
        app.open_prs.get_mut(&project).unwrap().list =
            vec![pr(43, "New", false), pr(42, "Fix login", false)];
        list_changed(&mut app);
        assert_eq!(view(&app).selected, 1, "#41 merged: its neighbour");
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(view(&app).scroll, 0, "a different pull request");
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/42"));
    }

    /// Enter opens the QUICK PROMPT for a PR SESSION on the row under the
    /// cursor, Shift+Tab the AGENT PRESETS picker for it and Tab the
    /// harness picker for it — the box's own three keys.
    #[test]
    fn the_launch_keys_start_a_pr_session_on_the_row() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            let expected = PrLaunch {
                url: "https://github.com/o/r/pull/41".into(),
                head: "branch-41".into(),
                number: 41,
            };
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Enter: expected the box, got {:?}", app.overlay);
            };
            let PromptKind::QuickPrompt(launch) = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(launch.pr.as_ref(), Some(&expected));
            assert_eq!(
                launch.target,
                QuickTarget::Worktree(WorktreeId("w-root".into()))
            );
            assert!(prompt.title.contains("PR #41"), "{}", prompt.title);

            // Shift+Tab in either spelling a terminal has for it.
            for preset_key in [key(KeyCode::BackTab), shifted(KeyCode::Tab)] {
                open(&mut app);
                handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
                handle_key(&mut app, preset_key, &mut Vec::new());
                let Some(Overlay::AgentPresets(presets)) = &app.overlay else {
                    panic!(
                        "Shift+Tab: expected the preset picker, got {:?}",
                        app.overlay
                    );
                };
                let back = presets.quick.as_ref().expect("a picker for a launch");
                assert_eq!(back.launch.pr.as_ref(), Some(&expected));
                assert!(!back.from_box, "no box to go back to");
            }
            // A PR SESSION runs in the pull request's own checkout: the
            // list's NEW WORKTREE `Tab` has nothing to flip, and says so.
            app.flash = None;
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Tab), &mut Vec::new());
            let Some(Overlay::AgentPresets(presets)) = &app.overlay else {
                panic!("Tab keeps the picker up, got {:?}", app.overlay);
            };
            assert!(presets.aim.is_none(), "{:?}", presets.aim);
            assert!(
                app.flash
                    .as_deref()
                    .is_some_and(|f| f.contains("own checkout")),
                "{:?}",
                app.flash
            );

            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, key(KeyCode::Tab), &mut Vec::new());
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("Tab: expected the harness picker, got {:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New PR session · #41"));
            assert!(!menu.items.is_empty());
        });
    }

    /// A project with no ROOT WORKTREE has nothing to address a PR SESSION
    /// to: the keys say so and the modal stays up.
    #[test]
    fn without_a_root_the_launch_keys_say_so() {
        pinned(|| {
            let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], false);
            for launch_key in [KeyCode::Enter, KeyCode::BackTab, KeyCode::Tab] {
                open(&mut app);
                app.flash = None;
                handle_key(&mut app, key(launch_key), &mut Vec::new());
                assert!(
                    matches!(&app.overlay, Some(Overlay::PullRequests(_))),
                    "{launch_key:?}: the modal stays"
                );
                assert_eq!(
                    app.flash.as_deref(),
                    Some("the project has no ROOT WORKTREE for this PR session"),
                    "{launch_key:?}"
                );
            }
        });
    }

    /// `Ctrl+c` opens the COMMENT BOX on the row, carrying the modal; Esc puts
    /// the modal back on the same pull request.
    #[test]
    fn c_opens_the_comment_box_and_esc_comes_back_to_the_row() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, ctrl('c'), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Ctrl+c: expected the comment box, got {:?}", app.overlay);
            };
            assert!(prompt.is_multiline());
            assert_eq!(prompt.title, "Comment on #41 Spike");
            let PromptKind::PrComment { number, back, .. } = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(*number, 41);
            assert_eq!(back.as_ref().map(|v| v.selected), Some(1));

            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Esc), &mut out);
            assert_eq!(view(&app).selected, 1, "back on #41");
        });
    }

    /// `Ctrl+y` is `Ctrl+c`: the grid's `y` (reply) as a chord, onto the
    /// same COMMENT BOX for the same row.
    #[test]
    fn ctrl_y_opens_the_comment_box_as_ctrl_c_does() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, ctrl('y'), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Ctrl+y: expected the comment box, got {:?}", app.overlay);
            };
            let PromptKind::PrComment { number, back, .. } = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(*number, 41);
            assert_eq!(
                back.as_ref().map(|v| v.query.as_str()),
                Some(""),
                "not typed into the filter"
            );
        });
    }

    /// `Enter` puts the QUICK PROMPT up over the modal, not in its place:
    /// the list stays on screen under the box, the box's Esc leaves the
    /// modal on the pull request it was opened on, and its launch closes
    /// the modal with it.
    #[test]
    fn enter_stacks_the_box_over_the_modal() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("expected the box, got {:?}", app.overlay);
            };
            let PromptKind::QuickPrompt(launch) = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert!(
                matches!(&launch.under, Some(ModalUnder::PullRequests(v)) if v.selected == 1),
                "{:?}",
                launch.under
            );
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
            term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer();
            let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
            assert!(
                screen.contains("Pull requests — demo"),
                "the modal under the box"
            );
            assert!(screen.contains("New session · PR #41"), "the box over it");
            assert!(
                screen.contains("Esc: back to pull requests"),
                "and says where Esc goes"
            );

            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Esc), &mut out);
            assert_eq!(view(&app).selected, 1, "Esc: back on #41");

            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            for c in "review it".chars() {
                crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Char(c)), &mut out);
            }
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Enter), &mut out);
            assert!(
                out.iter().any(|r| matches!(
                    r,
                    nebula_core::ClientRequest::CreatePrAgent { pr_url, .. }
                        if pr_url == "https://github.com/o/r/pull/41"
                )),
                "the PR session is created: {out:?}"
            );
            assert!(app.overlay.is_none(), "the launch closes the modal");
        });
    }

    fn detail(number: u64, title: &str) -> PrDetail {
        PrDetail {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: title.into(),
            state: crate::pull_request::STATE_OPEN.into(),
            is_draft: false,
            health: Health::default(),
            author: "webdevcody".into(),
            base: "main".into(),
            head: format!("branch-{number}"),
            additions: 3,
            deletions: 1,
            changed_files: 2,
            body: "Stops the login bounce.".into(),
            comments: vec![],
        }
    }

    /// The list reads like the group — a draft and a pull request GitHub
    /// says cannot merge wear their badge — and the pane reads the row
    /// under the cursor: its headline while the body is on its way, the
    /// body once it lands.
    #[test]
    fn it_draws_the_rows_and_reads_the_one_under_the_cursor() {
        let mut failing = pr(40, "Bump deps", false);
        failing.health = Health {
            conflicts: false,
            checks: Checks::Failing,
        };
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), failing, pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        let before = screen(&mut app, 120, 30);
        assert!(before.contains("Pull requests — demo (3)"), "{before}");
        assert!(before.contains("#42 Fix login"), "{before}");
        assert!(before.contains("failing"), "{before}");
        assert!(before.contains("draft"), "{before}");
        assert!(before.contains("Pull request #42"), "{before}");
        assert!(before.contains("reading it…"), "{before}");
        assert!(view(&app).list_area.height > 0, "rects written back");

        app.pr_detail.insert(
            "https://github.com/o/r/pull/42".into(),
            detail(42, "Fix login"),
        );
        let after = screen(&mut app, 120, 30);
        assert!(after.contains("Stops the login bounce."), "{after}");
        assert!(!after.contains("reading it…"), "{after}");

        app.pr_comment_inflight
            .insert("https://github.com/o/r/pull/42".into());
        let posting = screen(&mut app, 120, 30);
        assert!(posting.contains("posting your comment…"), "{posting}");
    }

    /// An empty list says whether GitHub is still being asked or has said
    /// nothing is open.
    #[test]
    fn an_empty_list_says_why() {
        let (mut app, project) = app_with(vec![], true);
        open(&mut app);
        assert!(screen(&mut app, 100, 20).contains("no open pull requests"));
        app.open_prs.remove(&project);
        assert!(screen(&mut app, 100, 20).contains("asking GitHub…"));
    }

    /// `Ctrl+o` and a click on the reading pane's `↗ open in browser` button run
    /// one open: the footer names where the browser went either way (INPUT
    /// PARITY), and the modal stays up. The button is drawn pinned right on
    /// the pane's top border, its rect written back for the click; the
    /// pointer resting on it is what `hover_crumb` holds, and a cell to its
    /// left is the frame's.
    #[test]
    fn o_and_the_browser_button_open_the_pull_request_the_same_way() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("↗ open in browser"), "{shot}");
        let button = view(&app).browser_area;
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
        handle_key(&mut app, ctrl('o'), &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/42"));
        app.flash = None;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut out);
        assert_eq!(app.flash.as_deref(), Some("opened github.com/o/r/pull/42"));
        assert!(
            matches!(app.overlay, Some(Overlay::PullRequests(_))),
            "the modal stays up"
        );
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            handle_key(app, key(KeyCode::Char(c)), &mut Vec::new());
        }
    }

    /// Typing narrows the rows the moment the modal is up — no key to
    /// press first — to the fuzzy matches, best first, the cursor on the
    /// best with its body asked for as any move's is, the count reading
    /// `matches/all`; the modal's own hotkey types too. ↑/↓ walk the
    /// matches alone. The first Esc clears the filter, the cursor staying
    /// on the row it found, and the second closes.
    #[test]
    fn typing_filters_the_rows_at_once_and_esc_clears_before_closing() {
        let (mut app, _) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", true),
            ],
            true,
        );
        open(&mut app);
        assert!(footer_hint().starts_with("type to filter"));
        type_str(&mut app, "login");
        assert_eq!(view(&app).query.as_str(), "login");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(2/3)"), "{shot}");
        assert!(
            !shot.contains("#41"),
            "the docs row is filtered out:\n{shot}"
        );
        let found = selected_pr(&app).expect("a row under the cursor");
        assert_ne!(found.number, 41);
        assert_eq!(
            pending_url(&app),
            Some(found.url.as_str()),
            "its body is asked for"
        );

        // `v` types, rather than closing.
        handle_key(&mut app, key(KeyCode::Char('v')), &mut Vec::new());
        assert_eq!(view(&app).query.as_str(), "loginv");
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
        handle_key(&mut app, key(KeyCode::Backspace), &mut Vec::new());

        let first = selected_pr(&app).unwrap().number;
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        let second = selected_pr(&app).unwrap().number;
        assert_ne!(first, second);
        assert_ne!(second, 41, "↓ walks the matches alone");
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(
            selected_pr(&app).unwrap().number,
            second,
            "and stops at the last one"
        );

        // The first Esc clears the filter, the cursor staying put; the
        // second closes.
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
        assert!(view(&app).query.is_empty());
        assert_eq!(
            selected_pr(&app).unwrap().number,
            second,
            "the row found keeps the cursor"
        );
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(3)"), "{shot}");
        assert!(shot.contains("type to filter…"), "{shot}");
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(app.overlay.is_none());
    }

    /// A filter nothing matches empties the list and says so — nothing
    /// under the cursor, no body asked for — and the row is back the
    /// moment the filter widens. Ctrl+u kills the typed filter, as in any
    /// line editor, and scrolls the pane only once there is none.
    #[test]
    fn a_filter_nothing_matches_says_so_and_leaves_the_cursor_put() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        select(&mut app, 1);
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        type_str(&mut app, "fix");
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the cursor goes to the best match"
        );
        type_str(&mut app, "zzz");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("no pull requests match"), "{shot}");
        assert!(shot.contains("(0/2)"), "{shot}");
        assert!(selected_pr(&app).is_none(), "nothing under the cursor");
        assert_eq!(pending_url(&app), None);
        for _ in 0..3 {
            handle_key(&mut app, key(KeyCode::Backspace), &mut Vec::new());
        }
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the row is back as the filter widens"
        );
        if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
            v.scroll = 3;
        }
        handle_key(&mut app, ctrl('u'), &mut Vec::new());
        assert!(view(&app).query.is_empty(), "Ctrl+u kills the typed filter");
        assert_eq!(view(&app).scroll, 3, "and does not scroll the pane");
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the row found keeps the cursor"
        );
        handle_key(&mut app, ctrl('u'), &mut Vec::new());
        assert_eq!(view(&app).scroll, 0, "with nothing typed, it scrolls");
    }

    /// A click on a row while a filter is typed picks that row — the row
    /// math counting the filter's matches, not the whole list — and the
    /// filter stays.
    #[test]
    fn a_row_click_counts_the_matches_not_the_list() {
        let (mut app, _) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", false),
            ],
            true,
        );
        open(&mut app);
        type_str(&mut app, "login");
        screen(&mut app, 120, 40);
        let list = view(&app).list_area;
        // The second visible row: the second match, whichever it is.
        let at = Position::new(list.x + 1, list.y + 1);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut Vec::new());
        assert_eq!(view(&app).query.as_str(), "login", "the filter is kept");
        let picked = selected_pr(&app).unwrap();
        let list = rows(&app, &view(&app).project);
        let visible = visible_rows("login", list);
        assert_eq!(picked.number, list[visible[1].0].number);
        assert_ne!(picked.number, 41);
    }

    /// A double-click on a row opens that pull request in the browser —
    /// the same open as `Ctrl+o` and the button (INPUT PARITY) — and the
    /// modal stays up on the row. A single click only selects, and two
    /// clicks on different rows are two single clicks.
    #[test]
    fn a_double_click_on_a_row_opens_it_in_the_browser() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        screen(&mut app, 120, 40);
        let list = view(&app).list_area;
        let click_at = |app: &mut App, row: u16| {
            let at = Position::new(list.x + 1, list.y + row);
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            };
            let mut out = Vec::new();
            handle_mouse(app, click, at, &mut out);
        };

        click_at(&mut app, 1);
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        assert_eq!(app.flash, None, "one click only selects");
        click_at(&mut app, 0);
        assert_eq!(selected_pr(&app).unwrap().number, 42);
        assert_eq!(app.flash, None, "a click on another row is a single click");
        click_at(&mut app, 0);
        assert_eq!(
            app.flash.as_deref(),
            Some("opened github.com/o/r/pull/42"),
            "the second click on the row opens it"
        );
        assert!(
            matches!(app.overlay, Some(Overlay::PullRequests(_))),
            "the modal stays up"
        );
        app.flash = None;
        click_at(&mut app, 0);
        assert_eq!(
            app.flash, None,
            "a double-click is spent: the third click starts over"
        );
    }

    /// A bracketed paste lands in the filter as one line and narrows the
    /// rows as typing it would; with no modal up it is not this one's.
    #[test]
    fn a_paste_lands_in_the_filter_as_one_line() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        assert!(paste(&mut app, "docs\npass"));
        assert_eq!(view(&app).query.as_str(), "docs pass");
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        app.overlay = None;
        assert!(!paste(&mut app, "x"));
    }

    /// A refresh that retires the row under the cursor while a filter is
    /// typed lands the cursor on the filter's next match, never on a row
    /// the filter hides.
    #[test]
    fn a_refresh_under_a_filter_lands_on_a_visible_row() {
        let (mut app, project) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", false),
            ],
            true,
        );
        open(&mut app);
        type_str(&mut app, "login");
        select(&mut app, 2);
        assert_eq!(selected_pr(&app).unwrap().number, 40);
        app.open_prs.get_mut(&project).unwrap().list =
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)];
        list_changed(&mut app);
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "not #41, which the filter hides"
        );
        screen(&mut app, 120, 40);
        assert_eq!(view(&app).selected, 0, "settled onto the row it shows");
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
    }

    /// The verbs the letters used to be are chords now: Ctrl+r asks
    /// GitHub again and Ctrl+g asks for the diff, each saying so in the
    /// footer, while the plain letters go to the filter.
    #[test]
    fn the_verb_chords_run_and_the_plain_letters_type() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        open(&mut app);
        handle_key(&mut app, ctrl('r'), &mut Vec::new());
        assert_eq!(app.flash.as_deref(), Some("refreshing pull requests…"));
        assert!(app.pr_refresh_requested);
        assert!(app.open_prs_lookup_due(&project));
        app.flash = None;
        handle_key(&mut app, ctrl('g'), &mut Vec::new());
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.starts_with("repo path missing on disk")),
            "Ctrl+g reaches the diff fetch: {:?}",
            app.flash
        );
        for letter in "rgoc".chars() {
            handle_key(&mut app, key(KeyCode::Char(letter)), &mut Vec::new());
        }
        assert_eq!(view(&app).query.as_str(), "rgoc");
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
    }

    /// No button on a frame too narrow to hold it clear of the title, and
    /// none with no pull request to open — and no stale rect either way.
    #[test]
    fn the_browser_button_is_left_off_a_narrow_frame_and_an_empty_list() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 60, 20);
        assert!(!shot.contains("open in browser"), "{shot}");
        assert_eq!(view(&app).browser_area, Rect::default());

        let (mut app, project) = app_with(vec![], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 120, 40);
        assert!(!shot.contains("open in browser"), "{shot}");
        assert_eq!(view(&app).browser_area, Rect::default());
    }
}
