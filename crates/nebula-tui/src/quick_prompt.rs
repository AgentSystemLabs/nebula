//! The QUICK PROMPT: the hotkey that opens a task box anywhere in the TUI
//! and launches an AGENT on what you type, without walking the NEW SESSION
//! PICKER first. The two are the two ways to start a session: `p` starts
//! one on a typed task, `n` starts one bare — a harness pick, and the first
//! prompt typed in the CLI — so the picker never ends in this box.
//!
//! What lives here is the launch spec — [`QuickLaunch`]: which AGENT KIND
//! and MODEL / EFFORT the `quick_prompt_kind` SETTING resolves to, which
//! AGENT PRESET (if any) wraps the text, and the [`QuickTarget`] it lands
//! in — the selected WORKTREE, or one that does not exist yet — plus the
//! two pickers that rewrite it for one launch (`Tab`, `Shift+Tab`), the
//! [`QuickReturn`] they carry so the round trip loses neither the spec
//! nor the typed text, and the `Ctrl+N` toggle that flips the target
//! between the selected checkout and a fresh worktree from any panel
//! (`toggle_new_worktree`). The dialog itself is an ordinary multi-line
//! `PromptDialog` (`PromptKind::QuickPrompt`) drawn by `ui::draw_overlay`,
//! and the create it ends in goes through `event_loop::create_agent` like
//! every other session, with the composed text as the STARTING PROMPT
//! (`event_loop::quick_launch` holds that last step). A box opened on a
//! PROJECT OPEN PRS GROUP row (`e` there, through the preset picker) is
//! the same box for a PR SESSION: it carries the pull request
//! ([`QuickLaunch::pr`]) and ends in a `CreatePrAgent` instead.

use crate::agent_presets::AgentPreset;
use crate::app::{App, Focus, Overlay, PromptKind};
use crate::config::{fit_effort, Config};
use crate::pull_request::PrLaunch;
use crate::text_input::TextInput;
use nebula_core::{AgentKind, ProjectId, WorktreeId};

/// Where a QUICK PROMPT launch lands.
#[derive(Debug, Clone, PartialEq)]
pub enum QuickTarget {
    /// The WORKTREE selected when the box opened.
    Worktree(WorktreeId),
    /// A WORKTREE that does not exist yet — `p` on the WORKTREES PANEL.
    /// Enter cuts `branch` off the PROJECT's fetched default base first
    /// (`ClientRequest::CreateWorktree`), and the launch follows into the
    /// checkout the DAEMON made once its Ack lands.
    NewWorktree { project: ProjectId, branch: String },
}

/// Everything one QUICK PROMPT will launch with. Resolved from the config
/// when the box opens and rewritten in place by the box's own pickers —
/// `Tab` (harness, then MODEL / EFFORT) and `Shift+Tab` (an AGENT PRESET).
/// Per-dialog: none of it is written back to CONFIG.JSON, so the next `p`
/// starts from the `quick_prompt_kind` SETTING again.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickLaunch {
    pub target: QuickTarget,
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The AGENT PRESET `Shift+Tab` picked: its prefix and postfix wrap
    /// the typed text into the STARTING PROMPT, and it pins the harness.
    /// `Tab` picking a harness clears it — a launch spec has one source.
    pub preset: Option<AgentPreset>,
    /// The GitHub issue this launch is for, when the box was opened from
    /// the ISSUES MODAL: named in the title, sent to the DAEMON as the
    /// session's persisted context (`CreateAgent::issue_url`), the name
    /// of the worktree `Ctrl+N` cuts, and the task when the box is sent
    /// empty. Kept across the box's pickers — the harness and the preset
    /// change what runs, not what it is for.
    pub issue: Option<crate::issues::IssueRef>,
    /// The pull request this launch is for, when the box was opened from
    /// a PROJECT OPEN PRS GROUP row (`e` on it): named in the title and
    /// the target row, and sent to the DAEMON as a PR SESSION
    /// (`CreatePrAgent`) — it runs in the PROJECT's checkout of the PR's
    /// head branch, reused when one is there and cut by the DAEMON
    /// otherwise, never in `target`, which only names the PROJECT (its
    /// ROOT WORKTREE). Kept across the box's pickers, as the issue is;
    /// `Ctrl+N` is refused, the checkout being the DAEMON's to pick.
    pub pr: Option<PrLaunch>,
}

/// What a picker for a QUICK PROMPT launch carries, so the trip loses
/// nothing: the launch as it stood, the text typed so far, and whether a
/// box was up to come back to.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickReturn {
    pub launch: QuickLaunch,
    pub text: String,
    /// Was the box up when the picker opened (`Tab` / `Shift+Tab` in it)?
    /// Esc puts the box back only then. A picker reached with no box up —
    /// `e` on a pull request or an issue — closes on Esc instead, as the
    /// manager does: it used to put up an empty box nobody asked for. A
    /// pick opens the box either way.
    pub from_box: bool,
}

impl QuickLaunch {
    /// The launch the `quick_prompt_kind` SETTING describes: that harness
    /// plus its own MODEL / EFFORT defaults from the AGENTS TAB, no preset.
    /// A Cursor effort is re-fitted to the configured family, as every
    /// other launch surface does — the daemon joins the two into one
    /// `--model` id.
    pub fn from_config(target: QuickTarget, cfg: &Config) -> Self {
        let kind = cfg.quick_prompt_kind();
        Self::of_kind(
            target,
            kind,
            None,
            cfg.default_model(kind),
            cfg.default_effort(kind),
            cfg,
        )
    }

    /// The launch a picked harness (and optional MODEL / EFFORT choice)
    /// describes: anything left unpicked falls back to that kind's
    /// configured default, and the effort is fitted to the model.
    pub fn of_kind(
        target: QuickTarget,
        kind: AgentKind,
        custom: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        cfg: &Config,
    ) -> Self {
        // Defaults resolve from the registry descriptor: its own model
        // default, and its effort default fitted to the model.
        let descriptor = cfg.effective_harness(kind, custom.as_deref());
        let model = model.or_else(|| descriptor.default_model().map(str::to_string));
        let effort = fit_effort(
            kind,
            model.as_deref(),
            effort.or_else(|| descriptor.default_effort().map(str::to_string)),
            custom.as_deref(),
        );
        Self {
            target,
            kind,
            custom,
            model,
            effort,
            preset: None,
            issue: None,
            pr: None,
        }
    }

    /// The same launch, for the pull request `pr` (or for none). What
    /// every picker's return trip does to the launch it rebuilt, so the
    /// PR survives a `Tab` or `Shift+Tab` pick as the issue does.
    pub fn with_pr(mut self, pr: Option<PrLaunch>) -> Self {
        self.pr = pr;
        self
    }

    /// The same launch, for `issue` (or for nothing, with `None`). What
    /// every picker's return trip does to the launch it rebuilt, so the
    /// issue survives a `Tab` or `Shift+Tab` pick.
    pub fn with_issue(mut self, issue: Option<crate::issues::IssueRef>) -> Self {
        self.issue = issue;
        self
    }

    /// The task Enter sends when the box is empty: an ISSUE SESSION's box
    /// may be sent as it is, the issue being the task. `None` for every
    /// other launch, where an empty box is a change of mind.
    pub fn default_task(&self) -> Option<String> {
        self.issue.as_ref().map(|issue| issue.default_task())
    }

    /// Does Enter on an empty box launch? Only a box an AGENT PRESET is
    /// on does — a preset's task is optional, so its empty box sends the
    /// prefix and postfix alone (nothing at all for a bare preset). With
    /// no preset an empty box is a change of mind: a session with no first
    /// prompt is the NEW SESSION PICKER's (`n`). (An ISSUE SESSION's empty
    /// box is `default_task`'s: the issue is the task.)
    pub fn launches_empty(&self) -> bool {
        self.preset.is_some()
    }

    /// The launch an AGENT PRESET describes: its harness, its pinned
    /// MODEL / EFFORT where it has them and that kind's defaults where it
    /// does not — the same resolution an AGENT PRESET launch from the
    /// SESSIONS PANEL does — and its prefix/postfix kept for the compose.
    pub fn of_preset(target: QuickTarget, preset: AgentPreset, cfg: &Config) -> Self {
        let mut launch = Self::of_kind(
            target,
            preset.kind,
            preset.custom_harness.clone(),
            preset.model.clone(),
            preset.effort.clone(),
            cfg,
        );
        launch.preset = Some(preset);
        launch
    }

    /// The checkout this launch is addressed to, to rewrite when a
    /// stand-in becomes the real row: None for a launch that cuts its
    /// own (`QuickTarget::NewWorktree`).
    pub fn worktree_mut(&mut self) -> Option<&mut WorktreeId> {
        match &mut self.target {
            QuickTarget::Worktree(worktree) => Some(worktree),
            QuickTarget::NewWorktree { .. } => None,
        }
    }

    /// The STARTING PROMPT this launch sends for `task`: the text itself,
    /// or the preset's prefix + task + postfix.
    pub fn compose(&self, task: &str) -> String {
        match &self.preset {
            Some(preset) => preset.compose(task),
            None => task.to_string(),
        }
    }

    /// The dialog's title: the issue (when the box is for one), the preset
    /// (when one is applied), the worktree Enter will cut first (when it
    /// is a new one) and the flags it will actually launch with, so Enter
    /// is never a surprise —
    /// `Quick prompt · reviewer (claude · opus · high)`,
    /// `Quick prompt · new worktree yellow-fox-jumps (claude)`,
    /// `Quick prompt · issue #15 · reviewer (claude · opus)`,
    /// `Quick prompt · PR #42 · reviewer (claude · opus)`.
    pub fn title(&self) -> String {
        let harness = self.custom.as_deref().unwrap_or_else(|| self.kind.as_str());
        let opts: Vec<&str> = std::iter::once(harness)
            .chain(self.model.as_deref())
            .chain(self.effort.as_deref())
            .collect();
        let mut head = vec!["Quick prompt".to_string()];
        if let Some(issue) = &self.issue {
            head.push(format!("issue #{}", issue.number));
        }
        if let Some(pr) = &self.pr {
            head.push(format!("PR #{}", pr.number));
        }
        if let Some(preset) = &self.preset {
            head.push(preset.name.clone());
        }
        if let QuickTarget::NewWorktree { branch, .. } = &self.target {
            head.push(format!("new worktree {branch}"));
        }
        format!("{} ({})", head.join(" · "), opts.join(" · "))
    }

    /// The line under the title: what Enter will send.
    pub fn label(&self) -> String {
        match (&self.preset, &self.issue) {
            (Some(preset), issue) => {
                let (sends, empty) = match (preset.has_wrapping(), issue) {
                    (true, Some(_)) => ("prefix + your task + postfix", "fix the issue"),
                    (true, None) => ("prefix + your task + postfix", "prefix + postfix only"),
                    (false, Some(_)) => ("sent as the first prompt", "fix the issue"),
                    (false, None) => ("sent as the first prompt", "start with no prompt"),
                };
                format!("{} — {sends} (empty = {empty})", preset.name)
            }
            (None, Some(issue)) => format!(
                "what should the agent do about #{}? (empty = fix the issue)",
                issue.number
            ),
            (None, None) => match &self.pr {
                Some(pr) => format!("what should the agent do about PR #{}?", pr.number),
                None => "what should the agent do?".into(),
            },
        }
    }

    /// Does Enter cut a fresh worktree before it launches? The box's frame
    /// turns green and its target row wears a NEW WORKTREE chip while so,
    /// whether the target came from the WORKTREES PANEL or from `Ctrl+N`.
    pub fn is_new_worktree(&self) -> bool {
        matches!(self.target, QuickTarget::NewWorktree { .. })
    }
}

/// The hotkey: open the task box for the selected WORKTREE. Unlike the
/// AGENT PRESETS list this does not ask for FOCUS on the SESSIONS PANEL —
/// the point of a quick prompt is that it works from wherever you are —
/// but it still needs a checkout to run in, so a PROJECT with no worktree
/// selected flashes instead.
///
/// The one exception is the WORKTREES PANEL: `p` there means "a fresh
/// worktree, then this task in it", whatever checkout the cursor is
/// parked on (the root, another checkout) and whether or not the
/// project's **Hide root worktree** setting has taken the root row out —
/// the checkout does not exist yet, so only the PROJECT has to be
/// selected. Its branch is the same random name the `n` prompt would
/// have offered.
///
/// A cursor parked on an OPEN PRS row — in that panel or, the row still
/// selected, from any other — makes the box a PR SESSION's, the one `e`
/// there hands back less the preset (`open_for_pr`): Enter sends a
/// `CreatePrAgent`, and the fresh worktree is the pull request's own,
/// on its head branch, its stand-in row up under the pull request from
/// the moment Enter is pressed. It used to be the random-branch checkout
/// above, launched with no PR context at all: the session had to check
/// the pull request out by hand, and its row only moved under the pull
/// request once the DAEMON's reconcile noticed the branch — the "slow
/// nesting" that was really a launch aimed at the wrong place.
pub(crate) fn open_quick_prompt(app: &mut App) {
    if app.selected_worktree_pr().is_some() {
        open_for_pr(app);
        return;
    }
    // An issue row is the ISSUES MODAL's row, in the panel: the box
    // carries the issue, into the project's root checkout.
    if app.selected_worktree_issue().is_some() {
        crate::issues::open_prompt_for_row(app);
        return;
    }
    if app.focus == Focus::Worktrees {
        let Some(project) = app.selected_project().map(|p| p.id.clone()) else {
            app.flash = Some("quick prompt: select a project first".into());
            return;
        };
        let branch = crate::branch_name::random_name(&app.project_branches(&project));
        open_for(app, QuickTarget::NewWorktree { project, branch });
        return;
    }
    let Some(worktree) = app.selected_worktree().map(|w| w.id.clone()) else {
        app.flash = Some("quick prompt: select a worktree first".into());
        return;
    };
    // The stand-in a previous `p` put up: git is still cutting it, and
    // the box would only be refused at Enter.
    if app.is_placeholder_worktree(&worktree) {
        app.flash = Some("quick prompt: worktree is still being created".into());
        return;
    }
    open_for(app, QuickTarget::Worktree(worktree));
}

/// Open the box for a known target, resolving the launch options now so
/// the title can name what Enter is about to start.
pub(crate) fn open_for(app: &mut App, target: QuickTarget) {
    let launch = QuickLaunch::from_config(target, &Config::load());
    crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch));
}

/// `p` with the Worktrees cursor on an OPEN PRS row: the box for a PR
/// SESSION on that pull request, titled for it (`Quick prompt · PR #42`),
/// its target row naming the PR and its head branch. Enter sends one
/// `CreatePrAgent` — the typed text its STARTING PROMPT — and, when the
/// PROJECT has no checkout on the head branch yet, puts the stand-in rows
/// up at once, nested under the pull request where the DAEMON's real row
/// will list (`create_agent`, through `placeholder::stage`). Nothing to
/// open when the project has no ROOT WORKTREE to address it to; the
/// footer says so.
fn open_for_pr(app: &mut App) {
    let Some(launch) = pr_launch(app) else {
        return;
    };
    // The text of a launch the DAEMON refused while another modal was up
    // (`App::parked_pr_prompt`): this box is where it was headed.
    let url = launch.pr.as_ref().map(|pr| pr.url.clone());
    let parked = match app.parked_pr_prompt.take() {
        Some((for_url, text)) if Some(&for_url) == url.as_ref() => Some(text),
        other => {
            app.parked_pr_prompt = other;
            None
        }
    };
    match parked {
        Some(text) => reopen(app, launch, &text),
        None => crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch)),
    }
}

/// The launch every PR SESSION box starts from — `p`'s and `e`'s alike:
/// the `quick_prompt_kind` SETTING's harness, the pull request under the
/// Worktrees cursor carried as `QuickLaunch::pr`, and the PROJECT's ROOT
/// WORKTREE as the target, which only names the project the create is
/// addressed to (as the `n` picker's does) — the DAEMON picks the
/// checkout, the PR head branch's own. None off a pull request row, and,
/// with a flash, when the project has no root to address it to.
fn pr_launch(app: &mut App) -> Option<QuickLaunch> {
    let pr = app.selected_worktree_pr().map(PrLaunch::of)?;
    let root = app.selected_project().and_then(|project| {
        app.tree
            .worktrees
            .iter()
            .find(|w| w.project_id == project.id && w.is_main)
            .map(|w| w.id.clone())
    });
    let Some(root) = root else {
        app.flash = Some("the project has no ROOT WORKTREE for this PR session".into());
        return None;
    };
    Some(QuickLaunch::from_config(QuickTarget::Worktree(root), &Config::load()).with_pr(Some(pr)))
}

/// The checkout a picker opened from the box is built against — the
/// `KindPicker` and the `AgentPresetsView` each carry one. For a WORKTREE
/// that does not exist yet it is the PROJECT's ROOT WORKTREE, which every
/// project has whether the panel shows it or not: in quick mode neither
/// picker launches into it, they hand the pick back and the launch keeps
/// its own `target`. None only if the project vanished meanwhile.
fn picker_context(app: &App, launch: &QuickLaunch) -> Option<WorktreeId> {
    match &launch.target {
        QuickTarget::Worktree(id) => Some(id.clone()),
        QuickTarget::NewWorktree { project, .. } => app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.project_id == project && w.is_main)
            .map(|w| w.id.clone()),
    }
}

/// Put the box back after one of its pickers — on a pick, with the new
/// launch, and on Esc with the one it left with. The text is restored
/// either way; that is the whole point of the round trip.
pub(crate) fn reopen(app: &mut App, launch: QuickLaunch, text: &str) {
    crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch));
    if let Some(crate::app::Overlay::Prompt(prompt)) = &mut app.overlay {
        prompt.input.insert_str(text);
    }
}

/// `Ctrl+N` in the box: flip this one launch between the selected
/// WORKTREE and a fresh one, from whichever panel `p` was pressed in —
/// the WORKTREES PANEL's "cut a worktree first" without walking over to
/// it, and the way back into the checkout under the cursor from there.
/// Flipping on mints the same random branch `n` would offer — or, for an
/// ISSUE SESSION, one named after the issue (`issue-15-fix-login`); flipping off
/// needs a real checkout under the cursor (not an OPEN PRS row, not a
/// stand-in git is still cutting) and says so while keeping the fresh one
/// otherwise. A PR SESSION's box has nothing to flip: the DAEMON picks
/// its checkout (the PR head branch's own), so the key only says so. The
/// box is rebuilt so its title and frame follow the target, with the
/// typed text and the caret exactly where they were.
pub(crate) fn toggle_new_worktree(app: &mut App, launch: QuickLaunch, input: TextInput) {
    if launch.pr.is_some() {
        app.flash =
            Some("quick prompt: a PR session runs in the pull request's own checkout".into());
        return;
    }
    let target = match &launch.target {
        QuickTarget::NewWorktree { .. } => match app.selected_worktree().map(|w| w.id.clone()) {
            None => Err("quick prompt: no worktree under the cursor — keeping the new one"),
            Some(worktree) if app.is_placeholder_worktree(&worktree) => {
                Err("quick prompt: worktree is still being created — keeping the new one")
            }
            Some(worktree) => Ok(QuickTarget::Worktree(worktree)),
        },
        QuickTarget::Worktree(id) => match app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map(|w| w.project_id.clone())
        {
            None => Err("quick prompt: worktree no longer exists"),
            Some(project) => {
                let taken = app.project_branches(&project);
                let branch = match &launch.issue {
                    Some(issue) => {
                        crate::branch_name::issue_name(issue.number, &issue.title, &taken)
                    }
                    None => crate::branch_name::random_name(&taken),
                };
                Ok(QuickTarget::NewWorktree { project, branch })
            }
        },
    };
    match target {
        Err(msg) => app.flash = Some(msg.into()),
        Ok(target) => {
            let launch = QuickLaunch { target, ..launch };
            crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch));
            if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
                prompt.input = input;
            }
        }
    }
}

/// The branch the box's target row names: the selected checkout's, or the
/// one Enter will cut. None only if the selected worktree vanished while
/// the box was up.
pub(crate) fn target_branch(app: &App, launch: &QuickLaunch) -> Option<String> {
    match &launch.target {
        QuickTarget::Worktree(id) => app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map(|w| w.branch.clone()),
        QuickTarget::NewWorktree { branch, .. } => Some(branch.clone()),
    }
}

/// `Tab` in the box: which harness this one launch uses. The same AGENT
/// KIND rows the NEW SESSION PICKER offers — so `→` drills into the same
/// MODEL / EFFORT submenus with the same TYPE-AHEAD — but the pick comes
/// back here instead of creating a session, and it clears any AGENT PRESET
/// (a launch spec has one source).
pub(crate) fn open_launch_picker(app: &mut App, back: QuickReturn) {
    let Some(context) = picker_context(app, &back.launch) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    crate::agent_picker::open_kind_picker(
        app,
        crate::agent_picker::KindPicker::quick_prompt(context, back),
    );
}

/// `Shift+Tab` in the box: the saved AGENT PRESETS as a picker. The list is
/// the one `e` opens in the SESSIONS PANEL, in picker mode — Enter adopts
/// the row's harness, MODEL / EFFORT and prefix/postfix for this launch and
/// Esc comes back unchanged, while `Ctrl+a` / `Ctrl+e` / `Ctrl+d` manage
/// the presets as they do there and come back to this picker. With none
/// saved yet it opens empty, on the same `Ctrl+a` hint the manager shows.
pub(crate) fn open_preset_picker(app: &mut App, back: QuickReturn) {
    let presets = crate::agent_presets::load();
    let selected = back
        .launch
        .preset
        .as_ref()
        .and_then(|p| presets.iter().position(|row| row.name == p.name))
        .unwrap_or(0);
    let Some(context) = picker_context(app, &back.launch) else {
        app.flash = Some("project no longer exists".into());
        return;
    };
    let mut view = crate::preset_overlays::AgentPresetsView::new(context, presets);
    view.selected = selected;
    view.quick = Some(back);
    app.overlay = Some(Overlay::AgentPresets(view));
}

/// `e` on a PROJECT OPEN PRS GROUP row: the saved AGENT PRESETS as a
/// picker for a PR SESSION on that pull request. The pick hands the
/// QUICK PROMPT box back with the preset applied and the PR carried
/// (`QuickLaunch::pr`) — or, for a `skip`-task preset, launches at once
/// — and Enter sends a `CreatePrAgent`: the DAEMON runs it in the
/// PROJECT's checkout of the PR's head branch, reusing one already there
/// and cutting one otherwise, with the PR URL and its work rule in the
/// system prompt and the preset's composed text as the first prompt. The
/// box's target is the PROJECT's ROOT WORKTREE, which only names the
/// PROJECT the create is addressed to (as the `n` picker's is). No box
/// was up when the list opened, so Esc closes it rather than putting up
/// an empty prompt nobody asked for.
pub(crate) fn open_preset_picker_for_pr(app: &mut App) {
    let Some(launch) = pr_launch(app) else {
        return;
    };
    open_preset_picker(
        app,
        QuickReturn {
            launch,
            text: String::new(),
            from_box: false,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> QuickTarget {
        QuickTarget::Worktree(WorktreeId::from("wt-1".to_string()))
    }

    fn new_worktree(branch: &str) -> QuickTarget {
        QuickTarget::NewWorktree {
            project: ProjectId::from("p-1".to_string()),
            branch: branch.into(),
        }
    }

    fn preset(name: &str, kind: AgentKind) -> AgentPreset {
        AgentPreset {
            name: name.into(),
            kind,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: String::new(),
            postfix: String::new(),
            skip_task: false,
        }
    }

    #[test]
    fn the_setting_picks_the_harness_and_its_own_model_and_effort() {
        let cfg = Config {
            quick_prompt_kind: "codex".into(),
            codex_model: "gpt-5.1-codex".into(),
            codex_effort: "high".into(),
            claude_model: "opus".into(),
            ..Config::default()
        };
        let launch = QuickLaunch::from_config(worktree(), &cfg);
        assert_eq!(launch.kind, AgentKind::Codex);
        assert_eq!(launch.model.as_deref(), Some("gpt-5.1-codex"));
        assert_eq!(launch.effort.as_deref(), Some("high"));
        assert!(launch.preset.is_none());
    }

    #[test]
    fn default_model_and_effort_pass_no_flags() {
        let launch = QuickLaunch::from_config(worktree(), &Config::default());
        assert_eq!(
            (launch.kind, launch.model, launch.effort),
            (AgentKind::Claude, None, None)
        );
    }

    /// A harness switched off on the AGENTS TAB after it was chosen would
    /// otherwise launch a kind the NEW SESSION PICKER no longer offers.
    #[test]
    fn a_disabled_harness_steps_on_to_an_enabled_one() {
        let cfg = Config {
            quick_prompt_kind: "claude".into(),
            claude_enabled: false,
            ..Config::default()
        };
        assert_eq!(
            QuickLaunch::from_config(worktree(), &cfg).kind,
            AgentKind::Codex
        );
    }

    /// A `Tab` pick names only the harness; the rest still comes from that
    /// kind's AGENTS TAB defaults, and a drilled-into submenu wins.
    #[test]
    fn a_picked_harness_fills_the_rest_from_its_own_defaults() {
        let cfg = Config {
            codex_model: "gpt-5.5".into(),
            codex_effort: "high".into(),
            ..Config::default()
        };
        let launch = QuickLaunch::of_kind(worktree(), AgentKind::Codex, None, None, None, &cfg);
        assert_eq!(launch.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(launch.effort.as_deref(), Some("high"));
        let launch = QuickLaunch::of_kind(
            worktree(),
            AgentKind::Codex,
            None,
            Some("gpt-5.1-codex".into()),
            Some("low".into()),
            &cfg,
        );
        assert_eq!(launch.model.as_deref(), Some("gpt-5.1-codex"));
        assert_eq!(launch.effort.as_deref(), Some("low"));
    }

    /// A preset pins what it names and inherits the rest, exactly as an
    /// AGENT PRESET launch from the SESSIONS PANEL does.
    #[test]
    fn a_preset_pins_what_it_names_and_wraps_the_task() {
        let cfg = Config {
            claude_effort: "high".into(),
            ..Config::default()
        };
        let reviewer = AgentPreset {
            model: Some("opus".into()),
            prefix: "Be strict.".into(),
            postfix: "Run the tests.".into(),
            ..preset("reviewer", AgentKind::Claude)
        };
        let launch = QuickLaunch::of_preset(worktree(), reviewer, &cfg);
        assert_eq!(launch.kind, AgentKind::Claude);
        assert_eq!(launch.model.as_deref(), Some("opus"));
        assert_eq!(
            launch.effort.as_deref(),
            Some("high"),
            "an unpinned effort falls back to the AGENTS TAB default"
        );
        assert_eq!(
            launch.compose("Fix auth"),
            "Be strict.\n\nFix auth\n\nRun the tests."
        );
    }

    #[test]
    fn the_title_and_label_name_the_launch() {
        let cfg = Config::default();
        let plain = QuickLaunch::of_kind(
            worktree(),
            AgentKind::Claude,
            None,
            Some("opus".into()),
            Some("high".into()),
            &cfg,
        );
        assert_eq!(plain.title(), "Quick prompt (claude · opus · high)");
        assert_eq!(plain.label(), "what should the agent do?");
        assert_eq!(plain.compose("do it"), "do it", "no preset, no wrapping");

        let wrapped = QuickLaunch::of_preset(
            worktree(),
            AgentPreset {
                prefix: "Be strict.".into(),
                ..preset("reviewer", AgentKind::Cursor)
            },
            &cfg,
        );
        assert_eq!(wrapped.title(), "Quick prompt · reviewer (cursor)");
        assert_eq!(
            wrapped.label(),
            "reviewer — prefix + your task + postfix (empty = prefix + postfix only)"
        );

        let bare = QuickLaunch::of_preset(worktree(), preset("scratch", AgentKind::Cursor), &cfg);
        assert_eq!(
            bare.label(),
            "scratch — sent as the first prompt (empty = start with no prompt)"
        );
    }

    /// An ISSUE SESSION's box names the issue first, offers the issue as
    /// the task when sent empty, and keeps the issue through a preset.
    #[test]
    fn an_issue_launch_names_the_issue_and_has_a_default_task() {
        let cfg = Config::default();
        let issue = crate::issues::IssueRef {
            url: "https://github.com/o/r/issues/15".into(),
            number: 15,
            title: "Fix login redirect".into(),
        };
        let plain = QuickLaunch::of_kind(worktree(), AgentKind::Claude, None, None, None, &cfg)
            .with_issue(Some(issue.clone()));
        assert_eq!(plain.title(), "Quick prompt · issue #15 (claude)");
        assert_eq!(
            plain.label(),
            "what should the agent do about #15? (empty = fix the issue)"
        );
        assert_eq!(
            plain.default_task().as_deref(),
            Some("Fix GitHub issue #15: Fix login redirect (https://github.com/o/r/issues/15)")
        );
        let wrapped = QuickLaunch::of_preset(
            new_worktree("issue-15-fix-login-redirect"),
            preset("reviewer", AgentKind::Cursor),
            &cfg,
        )
        .with_issue(Some(issue));
        assert_eq!(
            wrapped.title(),
            "Quick prompt · issue #15 · reviewer · new worktree issue-15-fix-login-redirect (cursor)"
        );
        assert_eq!(
            wrapped.label(),
            "reviewer — sent as the first prompt (empty = fix the issue)"
        );
        assert!(wrapped.default_task().is_some());
        let none = QuickLaunch::of_kind(worktree(), AgentKind::Claude, None, None, None, &cfg);
        assert_eq!(none.default_task(), None, "an empty ordinary box cancels");
    }

    /// A PR SESSION's box names the pull request, keeps it through a
    /// preset, and — unlike an issue's — offers no task when sent empty:
    /// the PR rides the system prompt, the preset's text is the task.
    #[test]
    fn a_pr_launch_names_the_pull_request_and_keeps_it_through_a_preset() {
        let cfg = Config::default();
        let pr = PrLaunch {
            url: "https://github.com/o/r/pull/42".into(),
            head: "fix-login".into(),
            number: 42,
        };
        let plain = QuickLaunch::of_kind(worktree(), AgentKind::Claude, None, None, None, &cfg)
            .with_pr(Some(pr.clone()));
        assert_eq!(plain.title(), "Quick prompt · PR #42 (claude)");
        assert_eq!(plain.label(), "what should the agent do about PR #42?");
        assert_eq!(plain.default_task(), None);
        assert!(!plain.launches_empty(), "no preset: an empty box cancels");
        assert!(!plain.is_new_worktree());

        let wrapped = QuickLaunch::of_preset(
            worktree(),
            AgentPreset {
                prefix: "Be strict.".into(),
                ..preset("reviewer", AgentKind::Cursor)
            },
            &cfg,
        )
        .with_pr(Some(pr.clone()));
        assert_eq!(wrapped.pr.as_ref(), Some(&pr));
        assert_eq!(wrapped.title(), "Quick prompt · PR #42 · reviewer (cursor)");
        assert_eq!(
            wrapped.label(),
            "reviewer — prefix + your task + postfix (empty = prefix + postfix only)"
        );
        assert!(wrapped.launches_empty(), "a preset's task is optional");
        assert_eq!(wrapped.compose("fix it"), "Be strict.\n\nfix it");
    }

    /// A launch into a worktree that does not exist yet says so — and
    /// names the branch Enter is about to cut — before the flags.
    #[test]
    fn the_title_names_the_worktree_a_launch_will_cut_first() {
        let cfg = Config::default();
        let fresh = QuickLaunch::of_kind(
            new_worktree("yellow-fox-jumps"),
            AgentKind::Claude,
            None,
            Some("opus".into()),
            None,
            &cfg,
        );
        assert_eq!(
            fresh.title(),
            "Quick prompt · new worktree yellow-fox-jumps (claude · opus)"
        );
        assert_eq!(fresh.label(), "what should the agent do?");

        let wrapped = QuickLaunch::of_preset(
            new_worktree("yellow-fox-jumps"),
            preset("reviewer", AgentKind::Cursor),
            &cfg,
        );
        assert_eq!(
            wrapped.title(),
            "Quick prompt · reviewer · new worktree yellow-fox-jumps (cursor)"
        );
    }

    /// An empty box is a change of mind — a session with no first prompt
    /// is what `n` starts — unless an AGENT PRESET is on it, whose task is
    /// optional, into a fresh worktree as much as into the selected one.
    #[test]
    fn only_a_preset_lets_an_empty_box_launch() {
        let cfg = Config::default();
        let plain = QuickLaunch::of_kind(worktree(), AgentKind::Claude, None, None, None, &cfg);
        assert_eq!(plain.title(), "Quick prompt (claude)");
        assert_eq!(plain.label(), "what should the agent do?");
        assert!(!plain.launches_empty(), "an empty box is a change of mind");

        let wrapped = QuickLaunch {
            preset: Some(preset("reviewer", AgentKind::Claude)),
            ..plain.clone()
        };
        assert!(wrapped.launches_empty());

        // `Ctrl+N` rebuilds the launch around a new target; neither
        // answer changes with it.
        let flipped = QuickLaunch {
            target: new_worktree("fix-login"),
            ..plain
        };
        assert!(!flipped.launches_empty());
        let flipped_wrapped = QuickLaunch {
            target: new_worktree("fix-login"),
            ..wrapped
        };
        assert!(flipped_wrapped.launches_empty());
    }
}
