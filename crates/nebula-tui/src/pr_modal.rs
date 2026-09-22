//! The PULL REQUESTS MODAL: the selected project's open pull requests,
//! listed down the left in the PROJECT OPEN PRS GROUP's order — newest
//! first, the drafts sunk below the finished ones — the one under the
//! cursor read on the right, and the ISSUES MODAL's ways to put an agent
//! on it: a QUICK PROMPT (`Enter` / `p`), one of the saved AGENT PRESETS
//! (`e`), or a bare harness pick (`n`, the group row's NEW SESSION
//! PICKER). Every one of them is a PR SESSION, launched exactly as the
//! group's row launches it (`quick_prompt::pr_launch_for`): the create is
//! a `CreatePrAgent`, the DAEMON runs the session in the project's
//! checkout of the pull request's head branch — reused when one is there,
//! cut otherwise, its stand-in rows up under the pull request from the
//! moment Enter is pressed — and the PR's URL rides the harness's context.
//!
//! `c` leaves a comment (the COMMENT BOX the row's `y` opens, which comes
//! back to the modal on its row), `g` reads the whole diff, `o` opens the
//! pull request in the browser, `r` asks GitHub again.
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
use nebula_core::{ProjectId, WorktreeId};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{clamp_selection, window_start, App, Overlay, PendingPrDetail, PromptKind};
use crate::keymap::{Action, KeyChord, Scope};
use crate::pr_preview::fit;
use crate::pull_request::{OpenPr, PrDetail};
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn};
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, panel_block, render_row, row_rect, truncate,
    SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

/// A list younger than this is what opening the modal shows, with no
/// second ask — the ISSUES MODAL's window, for the same reason: the list
/// the OPEN PRS beat landed moments ago *is* the answer. `r` asks
/// regardless.
pub(crate) const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
/// How long the cursor rests on a row before its body and conversation
/// are fetched — the pane's own debounce, so walking the list with `j`
/// fetches only the rows actually paused on.
const DETAIL_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
/// Left inset of the reading pane's own lines.
const INDENT: &str = " ";
/// The list column's share of the modal, and its floor.
const LIST_PCT: u16 = 38;
const MIN_LIST_W: u16 = 24;
/// Lines one wheel notch scrolls the reading pane.
const WHEEL_LINES: i32 = 3;

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
/// has rows.
fn selected_pr(app: &App) -> Option<OpenPr> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    rows(app, &view.project).get(view.selected).cloned()
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

/// `r`: ask for the list again now, and the selected pull request's body
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

/// The PROJECT's ROOT WORKTREE: what a PR SESSION create is addressed to.
fn root_worktree(app: &App, project: &ProjectId) -> Option<WorktreeId> {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.project_id == project && w.is_main)
        .map(|w| w.id.clone())
}

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

/// `Enter` / `p`: the QUICK PROMPT for a PR SESSION on the pull request.
/// The box goes up over the modal, which stays on screen under it: Esc
/// puts the modal back on the row (`QuickLaunch::under`), and the launch
/// closes it onto the new session's card.
fn open_prompt_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_pr_box(app, launch.with_under(under));
    }
}

/// `e`: one of the saved AGENT PRESETS as a PR SESSION on the pull
/// request. The pick hands the same box `Enter` opens back with the preset
/// applied; with no presets saved the footer says where to add one.
fn open_preset_for_selected(app: &mut App) {
    if let Some(launch) = launch_for_selected(app) {
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

/// `n`: the NEW SESSION PICKER's harness rows for a PR SESSION on the pull
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
    let Some(root) = root_worktree(app, &project) else {
        app.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
        return;
    };
    crate::agent_picker::open_kind_picker(
        app,
        crate::agent_picker::KindPicker::pr_session(root, &pr),
    );
}

/// `c`: the COMMENT BOX for the pull request under the cursor, carrying the
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

// ---- keys and mouse ----

/// Keys in the PULL REQUESTS MODAL.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    // The hotkey that opened the modal closes it, whatever it is bound to
    // — after the modal's own keys, so a rebind onto one of them can't
    // take it away.
    let toggles = app
        .keymap
        .lookup(Scope::Global, &KeyChord::from_event(&key))
        == Some(Action::PullRequests);
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let half = (view.view_height / 2).max(1) as i32;
    let page = view.view_height.max(1) as i32;
    let selected = view.selected as i64;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => close(app),
        KeyCode::Char('j') | KeyCode::Down if !shift => select(app, selected + 1),
        KeyCode::Char('k') | KeyCode::Up if !shift => select(app, selected - 1),
        // The reading pane scrolls on the ISSUES MODAL's keys.
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
        KeyCode::Char('n') => open_harness_picker_for_selected(app),
        // `y` is the panels' comment key on a pull request row.
        KeyCode::Char('c') | KeyCode::Char('y') => open_comment_for_selected(app),
        KeyCode::Char('g') => {
            if let Some(pr) = selected_pr(app) {
                crate::event_loop::request_pr_diff_for(app, pr.number, pr.url.clone(), pr.label());
            }
        }
        KeyCode::Char('o') => {
            if let Some(pr) = selected_pr(app) {
                if !crate::event_loop::open_url(&pr.url) {
                    app.flash = Some("could not open the browser".into());
                }
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => refresh(app),
        _ if toggles => close(app),
        _ => {}
    }
    app.dirty = true;
}

/// Mouse in the PULL REQUESTS MODAL: the wheel moves the cursor over the
/// list and scrolls the reading pane over it, a click on a row selects it
/// (a launch is `Enter`, not a click — the row is something to read
/// first), and a click outside closes (`overlay_close`); everything else
/// is swallowed.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, mouse_pos: Position) {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
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
            let first = view.window_start(list.height as usize);
            let len = app
                .open_prs
                .get(&view.project)
                .map_or(0, |open| open.list.len());
            if let Some(index) = crate::list_hit::row_at(list, first, len, mouse_pos) {
                select(app, index as i64);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

/// The footer's key line for the modal.
pub(crate) fn footer_hint() -> &'static str {
    "↑/↓: pull request  PgUp/PgDn ^d/^u: read  Enter/p: prompt an agent  e: preset  n: harness  c: comment  g: diff  o: browser  r: refresh  Esc: close"
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
/// else `draft` — so a row reads the way its group row does.
fn row_spans(pr: &OpenPr, budget: usize, th: Theme) -> Vec<Span<'static>> {
    let trouble = pr.trouble();
    let look = crate::pr_row::look(pr.standing(), trouble, th);
    let badge = match trouble {
        Some(trouble) => Some(trouble.badge()),
        None => pr.is_draft.then(|| pr.badge()),
    };
    let badge_w = badge.map_or(0, |b| b.chars().count());
    let text_budget = budget.saturating_sub(if badge_w > 0 { badge_w + 2 } else { 0 });
    let label = truncate(&pr.label(), text_budget);
    let number = format!("#{} ", pr.number);
    // A row with no title is its number alone.
    let title = label.strip_prefix(&number).unwrap_or_default().to_string();
    let used = number.chars().count() + title.chars().count();
    let number_color = if trouble.is_some() {
        look.label
    } else {
        th.dim
    };
    let mut spans = vec![
        Span::styled(number, Style::default().fg(number_color)),
        Span::styled(title, Style::default().fg(look.label)),
    ];
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
    let selected = view.selected.min(rows.len().saturating_sub(1));

    // ---- left: the list ----
    let title = format!(
        "Pull requests — {} ({}{})",
        view.project_name,
        rows.len(),
        if inflight { ", refreshing…" } else { "" }
    );
    let block = panel_block(&title, !backdrop, th).title_bottom(
        Line::from(Span::styled(
            // The launches; the footer spells out the rest.
            " Enter/p: prompt  e: preset  n: harness ",
            Style::default().fg(th.dim),
        ))
        .left_aligned(),
    );
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    if rows.is_empty() {
        let text = if inflight || !asked {
            "asking GitHub…"
        } else {
            "no open pull requests"
        };
        empty_list_row(f, list_inner, text, th);
    }
    let start = view.window_start(list_inner.height as usize);
    let budget = (list_inner.width as usize).saturating_sub(2);
    for (i, pr) in rows.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(list_inner, i - start) else {
            break;
        };
        render_row(
            f,
            row_area,
            row_spans(pr, budget, th),
            i == selected,
            !backdrop,
            th,
        );
    }

    // ---- right: the reading pane ----
    let current = rows.get(selected);
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
    let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
    f.render_widget(Paragraph::new(shown), body_inner);

    // Write-back (draw works on a clone): the rects the mouse hit-tests,
    // the pane's size for paging, and the clamped cursor and scroll.
    if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
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
    use crate::pull_request::{Checks, Health, PrLaunch};
    use crate::quick_prompt::QuickTarget;

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

    /// `j`/`k` walk the rows, each arming its own fetch; the hotkey, Esc
    /// and `q` all close.
    #[test]
    fn j_and_k_walk_the_rows_and_the_hotkey_closes() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
            true,
        );
        for close_key in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('v')] {
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Char('j')));
            assert_eq!(view(&app).selected, 1);
            assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/41"));
            handle_key(&mut app, key(KeyCode::Char('j')));
            assert_eq!(view(&app).selected, 1, "clamped at the last row");
            handle_key(&mut app, key(KeyCode::Char('k')));
            assert_eq!(view(&app).selected, 0);
            handle_key(&mut app, key(close_key));
            assert!(app.overlay.is_none(), "{close_key:?} closes");
        }
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
        handle_key(&mut app, key(KeyCode::Char('j')));
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

    /// `Enter` and `p` open the QUICK PROMPT for a PR SESSION on the row —
    /// the group row's launch, addressed to the project's root — `e` the
    /// AGENT PRESETS as a picker for the same launch, `n` the harness
    /// picker for it.
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
            for launch_key in [KeyCode::Enter, KeyCode::Char('p')] {
                open(&mut app);
                handle_key(&mut app, key(KeyCode::Char('j')));
                handle_key(&mut app, key(launch_key));
                let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                    panic!("{launch_key:?}: expected the box, got {:?}", app.overlay);
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
            }

            open(&mut app);
            handle_key(&mut app, key(KeyCode::Char('j')));
            handle_key(&mut app, key(KeyCode::Char('e')));
            let Some(Overlay::AgentPresets(presets)) = &app.overlay else {
                panic!("e: expected the preset picker, got {:?}", app.overlay);
            };
            let back = presets.quick.as_ref().expect("a picker for a launch");
            assert_eq!(back.launch.pr.as_ref(), Some(&expected));
            assert!(!back.from_box, "no box to go back to");

            open(&mut app);
            handle_key(&mut app, key(KeyCode::Char('j')));
            handle_key(&mut app, key(KeyCode::Char('n')));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("n: expected the harness picker, got {:?}", app.overlay);
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
            for launch_key in [KeyCode::Enter, KeyCode::Char('e'), KeyCode::Char('n')] {
                open(&mut app);
                app.flash = None;
                handle_key(&mut app, key(launch_key));
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

    /// `c` opens the COMMENT BOX on the row, carrying the modal; Esc puts
    /// the modal back on the same pull request.
    #[test]
    fn c_opens_the_comment_box_and_esc_comes_back_to_the_row() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Char('j')));
            handle_key(&mut app, key(KeyCode::Char('c')));
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("c: expected the comment box, got {:?}", app.overlay);
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
            handle_key(&mut app, key(KeyCode::Char('j')));
            handle_key(&mut app, key(KeyCode::Enter));
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

            handle_key(&mut app, key(KeyCode::Enter));
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
}
