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
use nebula_core::{Agent, AgentKind, WorktreeId};

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
    /// The row to start on: the surface's own ask (a QUICK PROMPT picker
    /// opens on its box's harness); None leaves it to REMEMBER HARNESS
    /// (Settings → Experimental — the last launch's), else the first
    /// row, which one not offered any more falls to as well.
    pub hover: Option<HarnessRow>,
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

    /// The QUICK PROMPT's `Tab` picker: every row hands the box back,
    /// and the cursor starts on the harness the box is already set to.
    /// `worktree` is the checkout the menu is built against, not where
    /// the launch lands — that stays the box's own `QuickLaunch::target`.
    pub fn quick_prompt(worktree: WorktreeId, back: QuickReturn) -> Self {
        Self {
            title: "Quick prompt agent".into(),
            worktree,
            pr: None,
            hover: Some(HarnessRow {
                kind: back.launch.kind,
                custom: back.launch.custom.clone(),
            }),
            quick: Some(Box::new(back)),
        }
    }
}

/// One launch row: a built-in kind, or a custom registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HarnessRow {
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
}

/// The harnesses the pickers offer — enabled built-ins plus usable custom
/// entries — or None, with the FLASH set, when a hand-edited config left
/// nothing to offer. An empty `ContextMenu` panics on Enter and `j`, so no
/// caller opens one.
pub(crate) fn enabled_harnesses_or_flash(app: &mut App, cfg: &Config) -> Option<Vec<HarnessRow>> {
    let rows: Vec<HarnessRow> = cfg
        .offered_harnesses()
        .into_iter()
        .map(|(kind, custom)| HarnessRow { kind, custom })
        .collect();
    if rows.is_empty() {
        app.flash = Some(NO_HARNESS_FLASH.into());
        return None;
    }
    Some(rows)
}

/// One `NewAgentOfKind` row per harness, labelled by `label`, every row
/// carrying the same launch context (`→` on any of them drills into that
/// kind's MODEL submenu with the context intact; customs offer no EFFORT
/// submenu — see `effort_choices`).
pub(crate) fn kind_rows(
    rows: &[HarnessRow],
    worktree: &WorktreeId,
    pr: Option<&PrLaunch>,
    quick: Option<&QuickReturn>,
    label: impl Fn(AgentKind, Option<&str>) -> String,
) -> Vec<MenuItem> {
    rows.iter()
        .map(|row| {
            MenuItem::new(
                label(row.kind, row.custom.as_deref()),
                MenuAction::NewAgentOfKind {
                    worktree: worktree.clone(),
                    kind: row.kind,
                    custom: row.custom.clone(),
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

/// The label a picker row shows: the registry entry's label for a known
/// id (falling back to the id when it carries none), the kind name
/// otherwise — including a custom id the registry no longer names.
pub(crate) fn harness_label(kind: AgentKind, custom: Option<&str>) -> String {
    let cfg = Config::load();
    match (kind, custom) {
        (AgentKind::Custom, Some(id))
            if cfg.harness_registry().iter().any(|entry| entry.id == id) =>
        {
            cfg.effective_harness_by_id(id).display_label().to_string()
        }
        _ => kind_label(kind).to_string(),
    }
}

/// Open `picker` as the OVERLAY, or FLASH when no harness is enabled.
pub(crate) fn open_kind_picker(app: &mut App, picker: KindPicker) {
    let cfg = Config::load();
    let Some(rows) = enabled_harnesses_or_flash(app, &cfg) else {
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
        .or_else(|| {
            cfg.remembered_kind()
                .map(|kind| HarnessRow { kind, custom: None })
        })
        .and_then(|wanted| {
            rows.iter().position(|row| {
                row.kind == wanted.kind && row.custom.as_deref() == wanted.custom.as_deref()
            })
        })
        .unwrap_or(0);
    let items = kind_rows(
        &rows,
        &worktree,
        pr.as_ref(),
        quick.as_deref(),
        harness_label,
    );
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
    let rows: Vec<HarnessRow> = Config::load()
        .offered_harnesses()
        .into_iter()
        .map(|(kind, custom)| HarnessRow { kind, custom })
        .collect();
    kind_rows(
        &rows,
        &worktree,
        Some(&PrLaunch::of(pr)),
        None,
        |kind, custom| format!("New {} session", harness_label(kind, custom)),
    )
}

/// The harness badge a session row wears: the built-in name, or the
/// custom entry's label (the id when the entry is gone, the kind name
/// when the row somehow names none).
pub(crate) fn session_harness_badge(agent: &Agent) -> String {
    match (agent.kind, agent.custom_harness.as_deref()) {
        (AgentKind::Custom, Some(id)) => Config::load()
            .effective_harness_by_id(id)
            .display_label()
            .to_string(),
        _ => agent.kind.as_str().to_string(),
    }
}

pub(crate) fn kind_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
        AgentKind::Pi => "Pi",
        AgentKind::Muse => "Muse",
        // Custom rows label through `harness_label` (the entry's own
        // label); this is only the fallback when its entry is gone.
        AgentKind::Custom => "Custom",
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

    /// Every row is the same launch context under a different harness, so
    /// a submenu drilled from any row keeps that context — including the
    /// custom registry id.
    #[test]
    fn kind_rows_carry_the_launch_context_into_every_row() {
        let worktree = WorktreeId("w1".into());
        let pr = PrLaunch::of(&open_pr());
        let harness_rows: Vec<HarnessRow> = AgentKind::ALL
            .into_iter()
            .filter(|kind| *kind != AgentKind::Custom)
            .map(|kind| HarnessRow { kind, custom: None })
            .chain([HarnessRow {
                kind: AgentKind::Custom,
                custom: Some("agy".into()),
            }])
            .collect();
        let rows = kind_rows(&harness_rows, &worktree, Some(&pr), None, |kind, custom| {
            format!("New {} session", harness_label(kind, custom))
        });
        assert!(rows.iter().any(|row| row.label == "New Codex session"));
        assert!(rows.iter().any(|row| row.label == "New Custom session"));
        for (row, expected) in rows.iter().zip(harness_rows.iter()) {
            assert!(
                matches!(
                    &row.action,
                    MenuAction::NewAgentOfKind {
                        worktree,
                        kind,
                        custom,
                        model: None,
                        effort: None,
                        cloud: false,
                        pr: Some(pr),
                        quick: None,
                    } if worktree.as_str() == "w1"
                        && *kind == expected.kind
                        && custom.as_deref() == expected.custom.as_deref()
                        && pr.url == PR_URL
                        && pr.head == PR_HEAD
                ),
                "{row:?}"
            );
            assert_eq!(row.action.submenu(), Some(SubmenuKind::Models));
        }
    }

    /// Custom entries ride after the built-ins under their own labels;
    /// disabled, invalid and (under the hide switch) missing ones stay
    /// out, and their rows carry the registry id into the launch.
    #[test]
    fn picker_lists_usable_custom_entries_after_the_builtins() {
        let json = r#"{"custom_harnesses": [
            {"id": "agy", "label": "Agy", "program": "agy"},
            {"id": "off", "label": "Off", "program": "off", "enabled": false},
            {"id": "broken", "label": "Broken", "program": ""}
        ]}"#;
        pinned(json, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            let names = labels(menu);
            assert_eq!(names.last(), Some(&"Agy"));
            assert!(!names.contains(&"Off"), "{names:?}");
            assert!(!names.contains(&"Broken"), "{names:?}");
            let agy = menu.items.iter().find(|item| item.label == "Agy").unwrap();
            assert!(matches!(
                &agy.action,
                MenuAction::NewAgentOfKind {
                    kind: AgentKind::Custom,
                    custom: Some(id),
                    ..
                } if id == "agy"
            ));
            assert_eq!(agy.action.submenu(), Some(SubmenuKind::Models));
        });
    }

    /// The Agents tab's per-entry Enabled row is the toggle now: flip
    /// it, save, and the picker drops the entry until it flips back.
    #[test]
    fn agents_tab_toggles_custom_entries_off_the_picker() {
        use crate::config::{locate_agent, HarnessField};
        let json = r#"{"custom_harnesses": [
            {"id": "agy", "label": "Agy", "program": "agy"}
        ]}"#;
        pinned(json, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert!(labels(menu).contains(&"Agy"));

            let mut cfg = Config::load();
            let (tab, row) = locate_agent("agy", HarnessField::Enabled).unwrap();
            cfg.cycle(tab, row, 0);
            cfg.save().unwrap();

            open_kind_picker(&mut app, KindPicker::new_session(worktree));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert!(!labels(menu).contains(&"Agy"), "{:?}", labels(menu));

            let mut cfg = Config::load();
            let (tab, row) = locate_agent("agy", HarnessField::Enabled).unwrap();
            cfg.cycle(tab, row, 0);
            cfg.save().unwrap();
            assert!(
                Config::load()
                    .offered_harnesses()
                    .contains(&(AgentKind::Custom, Some("agy".into()))),
                "flipping back re-offers the entry"
            );
        });
    }

    /// The three surfaces differ only in title, context and starting row;
    /// a harness switched off on the AGENTS TAB is missing from all of them.
    #[test]
    fn every_surface_opens_the_same_rows_minus_disabled_harnesses() {
        pinned(r#"{"codex_enabled": false}"#, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            // Every built-in harness but the one switched off, in the ALL
            // order. A bare Custom kind is never offered (entries come
            // from the registry), and the pinned config defines none.
            let offered: Vec<&str> = AgentKind::ALL
                .iter()
                .filter(|kind| **kind != AgentKind::Codex && **kind != AgentKind::Custom)
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
                    target: crate::quick_prompt::QuickTarget::Worktree(worktree.clone()),
                    kind: AgentKind::Cursor,
                    custom: None,
                    model: None,
                    effort: None,
                    preset: None,
                    issue: None,
                    pr: None,
                },
                text: "typed so far".into(),
            };
            open_kind_picker(&mut app, KindPicker::quick_prompt(worktree.clone(), back));
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

    /// REMEMBER HARNESS on: the NEW SESSION and PR SESSION pickers open on
    /// the last launch's harness — the `quick_prompt_kind` it wrote; off,
    /// on the first row whatever that setting says. A remembered harness
    /// switched off since steps to the first enabled one, and the QUICK
    /// PROMPT's picker still opens on its own box's harness.
    #[test]
    fn pickers_open_on_the_remembered_harness_only_while_the_switch_is_on() {
        let worktree = WorktreeId("w1".into());
        let pr = open_pr();
        fn hover_of(app: &App) -> usize {
            match &app.overlay {
                Some(Overlay::Menu(menu)) => menu.hover,
                other => panic!("expected a picker, got {other:?}"),
            }
        }

        pinned(r#"{"quick_prompt_kind": "cursor"}"#, || {
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            assert_eq!(
                hover_of(&app),
                0,
                "off: the quick prompt's harness is its own"
            );
        });

        pinned(
            r#"{"remember_harness": true, "quick_prompt_kind": "cursor"}"#,
            || {
                let mut app = App::new();
                open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
                assert_eq!(hover_of(&app), 2, "on: Cursor, the last launch's");
                open_kind_picker(&mut app, KindPicker::pr_session(worktree.clone(), &pr));
                assert_eq!(hover_of(&app), 2, "the PR SESSION picker too");

                let back = QuickReturn {
                    launch: QuickLaunch {
                        target: crate::quick_prompt::QuickTarget::Worktree(worktree.clone()),
                        kind: AgentKind::Codex,
                        custom: None,
                        model: None,
                        effort: None,
                        preset: None,
                        issue: None,
                        pr: None,
                    },
                    text: String::new(),
                };
                open_kind_picker(&mut app, KindPicker::quick_prompt(worktree.clone(), back));
                assert_eq!(
                    hover_of(&app),
                    1,
                    "the box's own harness outranks the remembered one"
                );
            },
        );

        pinned(
            r#"{"remember_harness": true, "quick_prompt_kind": "cursor", "cursor_enabled": false}"#,
            || {
                let mut app = App::new();
                open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
                let Some(Overlay::Menu(menu)) = &app.overlay else {
                    panic!("{:?}", app.overlay);
                };
                assert_eq!(labels(menu), ["Claude", "Codex", "Pi", "Muse"]);
                assert_eq!(
                    menu.hover, 0,
                    "a remembered harness switched off steps to the first enabled"
                );
            },
        );
    }

    /// Only a hand-edited config reaches an empty list: the picker flashes
    /// instead of opening, and a CONTEXT MENU just has no PR SESSION rows.
    #[test]
    fn no_harness_flashes_the_picker_and_empties_the_menu_rows() {
        pinned(
            r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false, "pi_enabled": false, "muse_enabled": false}"#,
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
