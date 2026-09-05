//! The QUICK PROMPT: the hotkey that opens a task box anywhere in the TUI
//! and launches an AGENT on what you type, without walking the NEW SESSION
//! PICKER first.
//!
//! What lives here is the launch spec — [`QuickLaunch`]: which AGENT KIND
//! and MODEL / EFFORT the `quick_prompt_kind` SETTING resolves to, which
//! AGENT PRESET (if any) wraps the text, and the WORKTREE it lands in —
//! plus the two pickers that rewrite it for one launch (`Tab`, `Shift+Tab`)
//! and the [`QuickReturn`] they carry so the round trip loses neither the
//! spec nor the typed text. The dialog itself is an ordinary multi-line
//! `PromptDialog` (`PromptKind::QuickPrompt`) drawn by `ui::draw_overlay`,
//! and the create it ends in goes through `event_loop::create_agent` like
//! every other session, with the composed text as the STARTING PROMPT.

use crate::agent_presets::AgentPreset;
use crate::app::{App, Overlay, PromptKind};
use crate::config::{fit_effort, Config};
use nebula_core::{AgentKind, WorktreeId};

/// Everything one QUICK PROMPT will launch with. Resolved from the config
/// when the box opens and rewritten in place by the box's own pickers —
/// `Tab` (harness, then MODEL / EFFORT) and `Shift+Tab` (an AGENT PRESET).
/// Per-dialog: none of it is written back to CONFIG.JSON, so the next `p`
/// starts from the `quick_prompt_kind` SETTING again.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickLaunch {
    pub worktree: WorktreeId,
    pub kind: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The AGENT PRESET `Shift+Tab` picked: its prefix and postfix wrap
    /// the typed text into the STARTING PROMPT, and it pins the harness.
    /// `Tab` picking a harness clears it — a launch spec has one source.
    pub preset: Option<AgentPreset>,
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
    pub fn from_config(worktree: WorktreeId, cfg: &Config) -> Self {
        let kind = cfg.quick_prompt_kind();
        Self::of_kind(
            worktree,
            kind,
            cfg.default_model(kind),
            cfg.default_effort(kind),
            cfg,
        )
    }

    /// The launch a picked harness (and optional MODEL / EFFORT choice)
    /// describes: anything left unpicked falls back to that kind's
    /// configured default, and the effort is fitted to the model.
    pub fn of_kind(
        worktree: WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
        cfg: &Config,
    ) -> Self {
        let model = model.or_else(|| cfg.default_model(kind));
        let effort = fit_effort(
            kind,
            model.as_deref(),
            effort.or_else(|| cfg.default_effort(kind)),
        );
        Self {
            worktree,
            kind,
            model,
            effort,
            preset: None,
        }
    }

    /// The launch an AGENT PRESET describes: its harness, its pinned
    /// MODEL / EFFORT where it has them and that kind's defaults where it
    /// does not — the same resolution an AGENT PRESET launch from the
    /// SESSIONS PANEL does — and its prefix/postfix kept for the compose.
    pub fn of_preset(worktree: WorktreeId, preset: AgentPreset, cfg: &Config) -> Self {
        let mut launch = Self::of_kind(
            worktree,
            preset.kind,
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

    /// The dialog's title: the preset (when one is applied) and the flags
    /// it will actually launch with, so Enter is never a surprise —
    /// `Quick prompt · reviewer (claude · opus · high)`.
    pub fn title(&self) -> String {
        let opts: Vec<&str> = std::iter::once(self.kind.as_str())
            .chain(self.model.as_deref())
            .chain(self.effort.as_deref())
            .collect();
        match &self.preset {
            Some(preset) => format!("Quick prompt · {} ({})", preset.name, opts.join(" · ")),
            None => format!("Quick prompt ({})", opts.join(" · ")),
        }
    }

    /// The line under the title: what Enter will send.
    pub fn label(&self) -> String {
        match &self.preset {
            Some(preset) if preset.has_wrapping() => {
                format!("{} — prefix + your task + postfix", preset.name)
            }
            Some(preset) => format!("{} — sent as the first prompt", preset.name),
            None => "what should the agent do?".into(),
        }
    }
}

/// The hotkey: open the task box for the selected WORKTREE. Unlike the
/// AGENT PRESETS list this does not ask for FOCUS on the SESSIONS PANEL —
/// the point of a quick prompt is that it works from wherever you are —
/// but it still needs a checkout to run in, so a PROJECT with no worktree
/// selected (or a cursor parked on an OPEN PRS row) flashes instead.
pub(crate) fn open_quick_prompt(app: &mut App) {
    let Some(worktree) = app.selected_worktree().map(|w| w.id.clone()) else {
        app.flash = Some("quick prompt: select a worktree first".into());
        return;
    };
    open_for(app, worktree);
}

/// Open the box for a known WORKTREE, resolving the launch options now so
/// the title can name what Enter is about to start.
pub(crate) fn open_for(app: &mut App, worktree: WorktreeId) {
    let launch = QuickLaunch::from_config(worktree, &Config::load());
    crate::event_loop::open_prompt(app, PromptKind::QuickPrompt(launch));
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

/// `Tab` in the box: which harness this one launch uses. The same AGENT
/// KIND rows the NEW SESSION PICKER offers — so `→` drills into the same
/// MODEL / EFFORT submenus with the same TYPE-AHEAD — but the pick comes
/// back here instead of creating a session, and it clears any AGENT PRESET
/// (a launch spec has one source).
pub(crate) fn open_launch_picker(app: &mut App, back: QuickReturn) {
    crate::agent_picker::open_kind_picker(app, crate::agent_picker::KindPicker::quick_prompt(back));
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
    let mut view =
        crate::preset_overlays::AgentPresetsView::new(back.launch.worktree.clone(), presets);
    view.selected = selected;
    view.quick = Some(back);
    app.overlay = Some(Overlay::AgentPresets(view));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> WorktreeId {
        WorktreeId::from("wt-1".to_string())
    }

    fn preset(name: &str, kind: AgentKind) -> AgentPreset {
        AgentPreset {
            name: name.into(),
            kind,
            model: None,
            effort: None,
            prefix: String::new(),
            postfix: String::new(),
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
        let launch = QuickLaunch::of_kind(worktree(), AgentKind::Codex, None, None, &cfg);
        assert_eq!(launch.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(launch.effort.as_deref(), Some("high"));
        let launch = QuickLaunch::of_kind(
            worktree(),
            AgentKind::Codex,
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
        assert_eq!(wrapped.label(), "reviewer — prefix + your task + postfix");

        let bare = QuickLaunch::of_preset(worktree(), preset("scratch", AgentKind::Cursor), &cfg);
        assert_eq!(bare.label(), "scratch — sent as the first prompt");
    }
}
