//! The QUICK PROMPT: the hotkey that opens a task box anywhere in the TUI
//! and launches an AGENT on what you type, without walking the NEW SESSION
//! PICKER first. The same box is that picker's own last step — `n`, a
//! harness, then this, where a name prompt used to stand
//! ([`QuickOrigin::NewSession`]).
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
//! (`event_loop::quick_launch` holds that last step).

use crate::agent_presets::AgentPreset;
use crate::app::{App, Focus, Overlay, PromptKind};
use crate::config::{fit_effort, Config};
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

/// Which surface put the box up. Both launch the same way; they differ in
/// what an empty box means and where FOCUS lands after Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickOrigin {
    /// The `p` hotkey, and the ISSUES MODAL's box: Enter on an empty box
    /// is a change of mind (an ISSUE SESSION aside), and the launch reads
    /// the `quick_prompt_focus` SETTING.
    Hotkey,
    /// The NEW SESSION PICKER's last step (`n`, then a harness), standing
    /// where the name prompt used to. Enter on an empty box still
    /// launches — with no STARTING PROMPT, the CLI's own input being the
    /// first prompt, as accepting the empty name prompt was — and the
    /// launch takes the TERMINAL PANE like every other picker-walked one.
    NewSession,
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
    /// Which surface opened the box — see [`QuickOrigin`]. Kept across the
    /// box's pickers and its `Ctrl+N`, as the issue is.
    pub origin: QuickOrigin,
}

/// What a picker opened *from* the box carries, so the trip loses nothing:
/// the launch as it stood (restored on Esc) and the text typed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickReturn {
    pub launch: QuickLaunch,
    pub text: String,
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
            origin: QuickOrigin::Hotkey,
        }
    }

    /// The same launch, for `issue` (or for nothing, with `None`). What
    /// every picker's return trip does to the launch it rebuilt, so the
    /// issue survives a `Tab` or `Shift+Tab` pick.
    pub fn with_issue(mut self, issue: Option<crate::issues::IssueRef>) -> Self {
        self.issue = issue;
        self
    }

    /// The same launch, from `origin`. The NEW SESSION PICKER sets it on
    /// the box it opens, and every picker's return trip keeps it, as
    /// `with_issue` keeps the issue.
    pub fn with_origin(mut self, origin: QuickOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// The task Enter sends when the box is empty: an ISSUE SESSION's box
    /// may be sent as it is, the issue being the task. `None` for every
    /// other launch, where an empty box is a change of mind.
    pub fn default_task(&self) -> Option<String> {
        self.issue.as_ref().map(|issue| issue.default_task())
    }

    /// Does Enter on an empty box launch? The NEW SESSION PICKER's box
    /// does, with no STARTING PROMPT, and so does any box an AGENT PRESET
    /// is on — a preset's task is optional, so its empty box sends the
    /// prefix and postfix alone (nothing at all for a bare preset). After
    /// `p` with no preset an empty box is a change of mind. (An ISSUE
    /// SESSION's empty box is `default_task`'s: the issue is the task.)
    pub fn launches_empty(&self) -> bool {
        self.origin == QuickOrigin::NewSession || self.preset.is_some()
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
    /// `Quick prompt · issue #15 · reviewer (claude · opus)` — and, for
    /// the NEW SESSION PICKER's box, `New session (claude · opus · high)`.
    pub fn title(&self) -> String {
        let harness = self
            .custom
            .as_deref()
            .unwrap_or_else(|| self.kind.as_str());
        let opts: Vec<&str> = std::iter::once(harness)
            .chain(self.model.as_deref())
            .chain(self.effort.as_deref())
            .collect();
        let mut head = vec![self.head().to_string()];
        if let Some(issue) = &self.issue {
            head.push(format!("issue #{}", issue.number));
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
            (None, None) => match self.origin {
                QuickOrigin::Hotkey => "what should the agent do?".into(),
                QuickOrigin::NewSession => {
                    "what should the agent do? (empty = start with no prompt)".into()
                }
            },
        }
    }

    /// The title's first words: which surface the box is.
    fn head(&self) -> &'static str {
        match self.origin {
            QuickOrigin::Hotkey => "Quick prompt",
            QuickOrigin::NewSession => "New session",
        }
    }

    /// The title of the box's `Tab` picker: `Quick prompt agent`, or
    /// `New session agent` over the NEW SESSION PICKER's own box.
    pub fn picker_title(&self) -> String {
        format!("{} agent", self.head())
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
/// selected (or a cursor parked on an OPEN PRS row) flashes instead.
///
/// The one exception is the WORKTREES PANEL: `p` there means "a fresh
/// worktree, then this task in it", whatever row the cursor is parked on
/// (the root, another checkout, an OPEN PRS row) and whether or not the
/// `hide_root_worktree` SETTING has taken the root row out — the checkout
/// does not exist yet, so only the PROJECT has to be selected. Its branch
/// is the same random name the `n` prompt would have offered.
pub(crate) fn open_quick_prompt(app: &mut App) {
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

/// The NEW SESSION PICKER's last step: the box for the harness (and the
/// MODEL / EFFORT) just picked, launching into the picker's worktree. It
/// stands where the name prompt used to — the session titles itself from
/// the prompt instead — and `Tab`, `Shift+Tab` and `Ctrl+N` work in it as
/// they do after `p`. `model` / `effort` are the picker's resolved choice;
/// `of_kind` only fills in what is still None.
pub(crate) fn open_for_new_session(
    app: &mut App,
    worktree: WorktreeId,
    kind: AgentKind,
    custom: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    cfg: &Config,
) {
    let launch =
        QuickLaunch::of_kind(QuickTarget::Worktree(worktree), kind, custom, model, effort, cfg)
            .with_origin(QuickOrigin::NewSession);
    crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch));
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
        prompt.input.insert_multiline_str(text);
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
/// otherwise. The box is rebuilt so its title and frame follow the
/// target, with the typed text and the caret exactly where they were.
pub(crate) fn toggle_new_worktree(app: &mut App, launch: QuickLaunch, input: TextInput) {
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
/// the one `e` opens in the SESSIONS PANEL, in pick-only mode — Enter
/// adopts the row's harness, MODEL / EFFORT and prefix/postfix for this
/// launch, Esc comes back unchanged, and `a`/`e`/`d` stay in the SESSIONS
/// PANEL where presets are managed. Nothing to pick leaves the box up.
pub(crate) fn open_preset_picker(app: &mut App, back: QuickReturn) {
    let presets = crate::agent_presets::load();
    if presets.is_empty() {
        app.flash = Some("no agent presets yet — press e in the Sessions panel to add one".into());
        return;
    }
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

    /// The NEW SESSION PICKER's box: titled for the picker, launching on
    /// an empty Enter — as any box an AGENT PRESET is on does — and keeping
    /// its origin through the rebuilds its pickers and `Ctrl+N` do.
    #[test]
    fn the_new_session_box_is_titled_for_the_picker_and_launches_empty() {
        let cfg = Config::default();
        let hotkey = QuickLaunch::of_kind(worktree(), AgentKind::Claude, None, None, None, &cfg);
        assert_eq!(hotkey.origin, QuickOrigin::Hotkey);
        assert_eq!(hotkey.title(), "Quick prompt (claude)");
        assert_eq!(hotkey.label(), "what should the agent do?");
        assert_eq!(hotkey.picker_title(), "Quick prompt agent");
        assert!(
            !hotkey.launches_empty(),
            "an empty hotkey box is a change of mind"
        );

        let picked = QuickLaunch::of_kind(
            worktree(),
            AgentKind::Claude,
            None,
            Some("opus".into()),
            Some("high".into()),
            &cfg,
        )
        .with_origin(QuickOrigin::NewSession);
        assert_eq!(picked.title(), "New session (claude · opus · high)");
        assert_eq!(
            picked.label(),
            "what should the agent do? (empty = start with no prompt)"
        );
        assert_eq!(picked.picker_title(), "New session agent");
        assert!(picked.launches_empty());

        // A preset's task is optional: its empty box launches, whichever
        // surface put the box up.
        let mut wrapped = picked.clone();
        wrapped.preset = Some(preset("reviewer", AgentKind::Claude));
        assert!(wrapped.launches_empty());
        let hotkey_preset = QuickLaunch {
            preset: wrapped.preset.clone(),
            ..hotkey.clone()
        };
        assert!(hotkey_preset.launches_empty());
        assert_eq!(
            wrapped.title(),
            "New session · reviewer (claude · opus · high)"
        );

        // `Ctrl+N` rebuilds the launch around a new target; the origin
        // rides along.
        let flipped = QuickLaunch {
            target: new_worktree("fix-login"),
            ..picked
        };
        assert_eq!(flipped.origin, QuickOrigin::NewSession);
        assert_eq!(
            flipped.title(),
            "New session · new worktree fix-login (claude · opus · high)"
        );
        assert!(flipped.launches_empty());
    }
}
