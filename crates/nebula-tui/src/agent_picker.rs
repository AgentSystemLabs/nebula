//! The one AGENT KIND picker behind every launch surface — the NEW SESSION
//! PICKER (`n` in the SESSIONS PANEL), the PR SESSION picker (`n` on a
//! PROJECT OPEN PRS GROUP row) and the QUICK PROMPT's `Tab` — plus the
//! per-harness rows a CONTEXT MENU on a PR row offers. Each is one
//! `ContextMenu` with a row per harness still enabled on the AGENTS TAB (a
//! disabled one is absent, not greyed), every row a
//! `MenuAction::NewAgentOfKind` carrying the surface's launch context, so
//! `→` drills into the same MODEL / EFFORT submenus everywhere.

use crate::app::{App, ContextMenu, MenuAction, MenuItem, Overlay};
use crate::config::Config;
use crate::pull_request::{OpenPr, PrLaunch};
use crate::quick_prompt::QuickReturn;
use nebula_core::{AgentKind, WorktreeId};

/// Shown instead of a picker when a hand-edited config has switched every
/// harness off (the AGENTS TAB refuses to turn off the last one).
pub(crate) const NO_HARNESS_FLASH: &str =
    "every harness is disabled — enable one in Settings › Agents";

/// What one kind picker opens with: its title, where its rows launch, and
/// the context every row carries.
#[derive(Debug, Clone)]
pub(crate) struct KindPicker {
    pub title: String,
    pub worktree: WorktreeId,
    /// OPEN PRS launch context (a PR SESSION picker).
    pub pr: Option<PrLaunch>,
    /// The QUICK PROMPT box owed back (its `Tab` picker).
    pub quick: Option<Box<QuickReturn>>,
    /// The row to start on; the first row when None or not offered.
    pub hover: Option<AgentKind>,
}

impl KindPicker {
    /// The NEW SESSION PICKER: plain rows into `worktree`.
    pub fn new_session(worktree: WorktreeId) -> Self {
        Self {
            title: "New session".into(),
            worktree,
            pr: None,
            quick: None,
            hover: None,
        }
    }

    /// The PR SESSION picker: every row carries the PR's URL and head
    /// branch. `worktree` is the PROJECT's ROOT WORKTREE — it names the
    /// PROJECT the create is addressed to, not where the session ends up;
    /// the DAEMON puts a PR SESSION in the head branch's own checkout.
    pub fn pr_session(worktree: WorktreeId, pr: &OpenPr) -> Self {
        Self {
            title: format!("New PR session · #{}", pr.number),
            worktree,
            pr: Some(PrLaunch::of(pr)),
            quick: None,
            hover: None,
        }
    }

    /// The QUICK PROMPT's `Tab` picker: every row hands the box back, and
    /// the cursor starts on the harness the box is already set to.
    pub fn quick_prompt(back: QuickReturn) -> Self {
        Self {
            title: "Quick prompt agent".into(),
            worktree: back.launch.worktree.clone(),
            pr: None,
            hover: Some(back.launch.kind),
            quick: Some(Box::new(back)),
        }
    }
}

/// The harnesses still enabled on the AGENTS TAB, or None — with the FLASH
/// set — when a hand-edited config disabled them all. An empty
/// `ContextMenu` panics on Enter and `j`, so no caller opens one.
pub(crate) fn enabled_kinds_or_flash(app: &mut App) -> Option<Vec<AgentKind>> {
    let kinds = Config::load().enabled_kinds();
    if kinds.is_empty() {
        app.flash = Some(NO_HARNESS_FLASH.into());
        return None;
    }
    Some(kinds)
}

/// One `NewAgentOfKind` row per harness, labelled by `label`, every row
/// carrying the same launch context (`→` on any of them drills into that
/// kind's MODEL / EFFORT submenus with the context intact).
pub(crate) fn kind_rows(
    kinds: &[AgentKind],
    worktree: &WorktreeId,
    pr: Option<&PrLaunch>,
    quick: Option<&QuickReturn>,
    label: impl Fn(AgentKind) -> String,
) -> Vec<MenuItem> {
    kinds
        .iter()
        .map(|&kind| {
            MenuItem::new(
                label(kind),
                MenuAction::NewAgentOfKind {
                    worktree: worktree.clone(),
                    kind,
                    model: None,
                    effort: None,
                    cloud: false,
                    pr: pr.cloned(),
                    quick: quick.map(|back| Box::new(back.clone())),
                },
            )
        })
        .collect()
}

/// Open `picker` as the OVERLAY, or FLASH when no harness is enabled.
pub(crate) fn open_kind_picker(app: &mut App, picker: KindPicker) {
    let Some(kinds) = enabled_kinds_or_flash(app) else {
        return;
    };
    let KindPicker {
        title,
        worktree,
        pr,
        quick,
        hover,
    } = picker;
    let hover = hover
        .and_then(|wanted| kinds.iter().position(|kind| *kind == wanted))
        .unwrap_or(0);
    let items = kind_rows(&kinds, &worktree, pr.as_ref(), quick.as_deref(), |kind| {
        kind_label(kind).to_string()
    });
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
    }));
}

/// The PR SESSION rows a CONTEXT MENU on a PROJECT OPEN PRS GROUP row
/// offers — `New Claude session`, `New Codex session`, … — one per enabled
/// harness, and none (no FLASH: the menu's other verbs still apply) when
/// every harness is off.
pub(crate) fn pr_session_menu_rows(worktree: WorktreeId, pr: &OpenPr) -> Vec<MenuItem> {
    let kinds = Config::load().enabled_kinds();
    kind_rows(&kinds, &worktree, Some(&PrLaunch::of(pr)), None, |kind| {
        format!("New {} session", kind_label(kind))
    })
}

pub(crate) fn kind_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
        AgentKind::Pi => "Pi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SubmenuKind;
    use crate::quick_prompt::QuickLaunch;

    const PR_URL: &str = "https://github.com/o/r/pull/7";
    const PR_HEAD: &str = "attach-links";

    /// Pin the config to a temp file holding `json`: every picker reads
    /// `Config::load`, and the dev's real file must stay out of it.
    fn pinned<T>(json: &str, f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        crate::config::with_config_path(path, f)
    }

    fn open_pr() -> OpenPr {
        OpenPr {
            number: 7,
            title: "Attach links".into(),
            url: PR_URL.into(),
            is_draft: false,
            head: PR_HEAD.into(),
        }
    }

    fn labels(menu: &ContextMenu) -> Vec<&str> {
        menu.items.iter().map(|item| item.label.as_str()).collect()
    }

    /// Every row is the same launch context under a different kind, so a
    /// submenu drilled from any row keeps that context.
    #[test]
    fn kind_rows_carry_the_launch_context_into_every_row() {
        let worktree = WorktreeId("w1".into());
        let pr = PrLaunch::of(&open_pr());
        let rows = kind_rows(&AgentKind::ALL, &worktree, Some(&pr), None, |kind| {
            format!("New {} session", kind_label(kind))
        });
        let names: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        let expected: Vec<String> = AgentKind::ALL
            .iter()
            .map(|kind| format!("New {} session", kind_label(*kind)))
            .collect();
        assert_eq!(names, expected);
        assert!(names.contains(&"New Codex session"));
        for (row, expected) in rows.iter().zip(AgentKind::ALL) {
            assert!(
                matches!(
                    &row.action,
                    MenuAction::NewAgentOfKind {
                        worktree,
                        kind,
                        model: None,
                        effort: None,
                        cloud: false,
                        pr: Some(pr),
                        quick: None,
                    } if worktree.as_str() == "w1"
                        && *kind == expected
                        && pr.url == PR_URL
                        && pr.head == PR_HEAD
                ),
                "{row:?}"
            );
            assert_eq!(row.action.submenu(), Some(SubmenuKind::Models));
        }
    }

    /// The three surfaces differ only in title, context and starting row;
    /// a harness switched off on the AGENTS TAB is missing from all of them.
    #[test]
    fn every_surface_opens_the_same_rows_minus_disabled_harnesses() {
        pinned(r#"{"codex_enabled": false}"#, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            // Every harness but the one switched off, in the ALL order.
            let offered: Vec<&str> = AgentKind::ALL
                .iter()
                .filter(|kind| **kind != AgentKind::Codex)
                .map(|kind| kind_label(*kind))
                .collect();
            assert!(offered.starts_with(&["Claude", "Cursor"]), "{offered:?}");

            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New session"));
            assert_eq!(labels(menu), offered);
            assert_eq!(menu.hover, 0);

            open_kind_picker(
                &mut app,
                KindPicker::pr_session(worktree.clone(), &open_pr()),
            );
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New PR session · #7"));
            assert_eq!(labels(menu), offered);
            assert!(menu.items.iter().all(|item| matches!(
                &item.action,
                MenuAction::NewAgentOfKind { pr: Some(pr), quick: None, .. }
                    if pr.url == PR_URL && pr.head == PR_HEAD
            )));

            let back = QuickReturn {
                launch: QuickLaunch {
                    worktree: worktree.clone(),
                    kind: AgentKind::Cursor,
                    model: None,
                    effort: None,
                    preset: None,
                },
                text: "typed so far".into(),
            };
            open_kind_picker(&mut app, KindPicker::quick_prompt(back));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Quick prompt agent"));
            assert_eq!(labels(menu), offered);
            assert_eq!(menu.hover, 1, "starts on the box's own harness");
            assert!(menu.items.iter().all(|item| matches!(
                &item.action,
                MenuAction::NewAgentOfKind { pr: None, quick: Some(back), .. }
                    if back.text == "typed so far"
            )));

            let rows = pr_session_menu_rows(worktree, &open_pr());
            let names: Vec<String> = rows.iter().map(|row| row.label.clone()).collect();
            let expected: Vec<String> = offered
                .iter()
                .map(|label| format!("New {label} session"))
                .collect();
            assert_eq!(names, expected);
        });
    }

    /// Only a hand-edited config reaches an empty list: the picker flashes
    /// instead of opening, and a CONTEXT MENU just has no PR SESSION rows.
    #[test]
    fn no_harness_flashes_the_picker_and_empties_the_menu_rows() {
        pinned(
            r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false, "pi_enabled": false}"#,
            || {
                let worktree = WorktreeId("w1".into());
                let mut app = App::new();
                open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
                assert!(app.overlay.is_none(), "{:?}", app.overlay);
                assert_eq!(app.flash.as_deref(), Some(NO_HARNESS_FLASH));
                assert!(pr_session_menu_rows(worktree, &open_pr()).is_empty());
            },
        );
    }
}
