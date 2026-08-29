//! The `/` PALETTE: one fuzzy search over every WORKSPACE, PROJECT,
//! WORKTREE, SESSION and open pull request nebula knows about, in *every*
//! workspace — the "jump to anything" tool. Its rows are built here from
//! the tree; `event_loop.rs` handles the keys and the jump, `ui.rs` draws
//! the rows.

use crate::app::{
    clamp_selection, last_interaction_ms, now_ms, project_recency, project_rollup, project_unseen,
    window_start, workspace_recency, workspace_rollup, workspace_unseen, worktree_recency,
    worktree_rollup, worktree_unseen, OpenPrs, Tree,
};
use crate::text_input::TextInput;
use nebula_core::{Agent, AgentId, AgentStatus, Project, ProjectId, WorkspaceId, WorktreeId};
use ratatui::layout::Rect;
use std::collections::HashMap;

/// What a `/` palette row jumps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteTarget {
    /// A whole workspace: picking it switches this instance to it, the
    /// same as the `w` switcher's Enter.
    Workspace(WorkspaceId),
    Project(ProjectId),
    Worktree(WorktreeId),
    Session(AgentId),
    /// An open pull request on some project's repo, addressed by URL — the
    /// only identity it has, since nothing about a PR is stored. Picking it
    /// opens a browser instead of moving any panel cursor.
    PullRequest(String),
}

/// Where a `/` row sits before the query has said anything — the tiers of
/// the PALETTE's attention order, best first. A SESSION waiting on you
/// (NEEDS FEEDBACK) comes first, then one mid-turn (RUNNING), then one that
/// finished a turn nobody has read (UNSEEN); every other row — read and
/// never-run sessions, and every workspace, project, worktree and pull
/// request — sorts under those in RECENCY ORDER, so the checkout you were
/// just in is the first thing after what needs you. ARCHIVED rows sink
/// below even the never-run ones, as in the SESSIONS PANEL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteTier {
    NeedsFeedback,
    Running,
    Unseen,
    Rest,
    Archived,
}

/// One searchable row of the `/` palette. `text` is both the string the
/// fuzzy filter runs over and the string rendered after the kind badge, so
/// match highlighting always lines up. Every row carries its full path
/// from the workspace down — `workspace` for workspaces,
/// `workspace/project` for projects, `workspace/project/branch` for
/// worktrees, `workspace/project/branch/name` for sessions — so a query
/// can narrow by any ancestor, a workspace name included.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub target: PaletteTarget,
    pub text: String,
    pub archived: bool,
    /// The status this row's panel row would show: a rollup for projects
    /// and worktrees, its own status for a session. Drives the glyph color
    /// and the text sweep, so a running session reads as running in the
    /// palette too. Refreshed by [`Palette::rebuild`] as upserts land.
    pub status: Option<AgentStatus>,
    /// Whether anything under this row finished a turn nobody has read.
    /// Splits a finished dot green (read) from violet (waiting on you),
    /// exactly as the panel rows do.
    pub unseen: bool,
    /// The attention tier this row sorts into with an empty query, and the
    /// tiebreak between equal scores once there is one. See [`PaletteTier`].
    pub tier: PaletteTier,
    /// The row's RECENCY ORDER stamp: [`last_interaction_ms`] for a session
    /// (a working one counts as now), the newest stamp under it for a
    /// workspace, project or worktree, `archived_at` for an archived row,
    /// nothing for a pull request. 0 sorts last within its tier.
    pub interacted: i64,
}

/// One visible palette row: an index into `items` plus the char positions of
/// `text` the query matched, for highlighting.
#[derive(Debug, Clone)]
pub struct PaletteMatch {
    pub item: usize,
    pub positions: Vec<usize>,
}

/// Fuzzy-search palette over every workspace, project, worktree, and
/// session (`/`), across all workspaces — not just the open one.
#[derive(Debug, Clone)]
pub struct Palette {
    pub items: Vec<PaletteItem>,
    /// Type-to-filter query over `items` texts; always live.
    pub query: TextInput,
    /// Visible rows: `items` narrowed by `query`, best matches first, ties
    /// — and the whole list, when the query is empty — in the attention
    /// order of [`PaletteTier`] then most recent interaction.
    pub matches: Vec<PaletteMatch>,
    /// Index into `matches` (not `items`).
    pub selected: usize,
    /// Whether Enter (and a click) on a session row attaches to it, or only
    /// lands on its Sessions-panel row. Snapshot of the config setting at
    /// open time; Ctrl+O / Ctrl+F pick explicitly either way.
    pub enter_attaches: bool,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
}

impl Palette {
    pub fn new(
        tree: &Tree,
        show_archived: bool,
        enter_attaches: bool,
        open_prs: &HashMap<ProjectId, OpenPrs>,
    ) -> Self {
        let mut palette = Self {
            items: build_palette_items(tree, show_archived, open_prs),
            query: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            enter_attaches,
            area: Rect::default(),
            list_area: Rect::default(),
        };
        palette.apply_filter();
        palette
    }

    /// Re-derive `items` after the tree changed under an open palette,
    /// keeping the query — and the cursor: agent status flips arrive as
    /// upserts every few seconds, and a rebuild must not yank the user's
    /// ↑/↓ position to the top. The selection follows its target's row;
    /// only a vanished target falls back to the best match.
    pub fn rebuild(
        &mut self,
        tree: &Tree,
        show_archived: bool,
        open_prs: &HashMap<ProjectId, OpenPrs>,
    ) {
        let keep = self.selected_target().cloned();
        self.items = build_palette_items(tree, show_archived, open_prs);
        self.apply_filter();
        if let Some(target) = keep {
            if let Some(row) = self
                .matches
                .iter()
                .position(|m| self.items[m.item].target == target)
            {
                self.selected = row;
            }
        }
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }

    /// Clamped absolute selection in the filtered list.
    pub fn select(&mut self, index: i64) {
        self.selected = clamp_selection(index, self.matches.len());
    }

    /// The jump target behind the current selection, if any row is visible.
    pub fn selected_target(&self) -> Option<&PaletteTarget> {
        Some(
            &self
                .items
                .get(self.matches.get(self.selected)?.item)?
                .target,
        )
    }

    /// Recompute `matches` from `query` and reset the selection to the top
    /// row. Best matches first; the attention order breaks ties and is the
    /// whole order when the query is empty, so `/` `Enter` lands on the
    /// session that needs you before anything else.
    pub fn apply_filter(&mut self) {
        let rank = attention_rank(&self.items);
        self.matches = crate::fuzzy::rank_by(
            &self.query,
            self.items.iter().map(|i| i.text.as_str()),
            |i, _| rank[i],
        )
        .into_iter()
        .map(|(item, positions)| PaletteMatch { item, positions })
        .collect();
        self.selected = 0;
    }
}

/// Each item's position in the attention order: tier first, then most
/// recently interacted, then build order — which keeps a workspace over its
/// projects over their worktrees over their sessions when they share a
/// stamp, and never-run rows in tree order with the open workspace first.
fn attention_rank(items: &[PaletteItem]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (items[i].tier, std::cmp::Reverse(items[i].interacted), i));
    let mut rank = vec![0; items.len()];
    for (pos, i) in order.into_iter().enumerate() {
        rank[i] = pos;
    }
    rank
}

/// The tier a session row sorts into — its own status, read against the
/// UNSEEN flag the DONE BADGE counts; an archived row sinks whatever its
/// last status was.
fn session_tier(a: &Agent) -> PaletteTier {
    if a.archived {
        return PaletteTier::Archived;
    }
    match a.status {
        AgentStatus::NeedsFeedback => PaletteTier::NeedsFeedback,
        AgentStatus::Running => PaletteTier::Running,
        AgentStatus::Finished if a.unseen => PaletteTier::Unseen,
        _ => PaletteTier::Rest,
    }
}

/// Every jumpable entity, across every workspace: the workspaces
/// themselves, then each one's projects in tree order, then their
/// worktrees, then their sessions, then the open pull requests nebula has
/// fetched. Archived sessions appear only when the archived toggle is on
/// (the Sessions panel rule).
///
/// This is the build order — what `matches` falls back to among rows with
/// the same tier and stamp; the open workspace comes first so never-run
/// rows favor what's on screen. The order the user sees is
/// [`attention_rank`]'s. Every row's text is prefixed with its workspace,
/// which is both what keeps the paths unambiguous once two workspaces can
/// hold the same project name and what lets a query cross over (`/` then
/// the other workspace's name).
fn build_palette_items(
    tree: &Tree,
    show_archived: bool,
    open_prs: &HashMap<ProjectId, OpenPrs>,
) -> Vec<PaletteItem> {
    let now = now_ms();
    let mut items = Vec::new();
    for id in palette_workspace_order(tree) {
        // A project can outlive knowledge of its workspace — its upsert can
        // land before the workspace's, and a workspace can go while a stale
        // project row is still in the tree. Such a project still belongs in
        // `/` (vanishing from the find-anything tool is the worst failure
        // it has); it just has no name to path it under, and no row of its
        // own to jump to.
        let workspace = tree.workspaces.iter().find(|w| w.id == id);
        if let Some(ws) = workspace {
            items.push(PaletteItem {
                target: PaletteTarget::Workspace(ws.id.clone()),
                text: ws.name.clone(),
                archived: false,
                status: workspace_rollup(tree, &ws.id),
                unseen: workspace_unseen(tree, &ws.id) > 0,
                tier: PaletteTier::Rest,
                interacted: workspace_recency(tree, &ws.id, now).interacted,
            });
        }
        let at = match workspace {
            Some(ws) => format!("{}/", ws.name),
            None => String::new(),
        };
        let projects: Vec<&Project> = tree
            .projects
            .iter()
            .filter(|p| p.workspace_id == id)
            .collect();
        // Within a workspace the kinds stay grouped project → worktree →
        // session, so a bare query still ranks the shallowest match first.
        for p in &projects {
            items.push(PaletteItem {
                target: PaletteTarget::Project(p.id.clone()),
                text: format!("{at}{}", p.name),
                archived: false,
                status: project_rollup(tree, &p.id),
                unseen: project_unseen(tree, &p.id) > 0,
                tier: PaletteTier::Rest,
                interacted: project_recency(tree, &p.id, now).interacted,
            });
        }
        for p in &projects {
            for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
                items.push(PaletteItem {
                    target: PaletteTarget::Worktree(w.id.clone()),
                    text: format!("{at}{}/{}", p.name, w.branch),
                    archived: false,
                    status: worktree_rollup(tree, &w.id),
                    unseen: worktree_unseen(tree, &w.id) > 0,
                    tier: PaletteTier::Rest,
                    interacted: worktree_recency(tree, &w.id, now).interacted,
                });
            }
        }
        for p in &projects {
            for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
                for a in tree.agents.iter().filter(|a| a.worktree_id == w.id) {
                    if a.archived && !show_archived {
                        continue;
                    }
                    items.push(PaletteItem {
                        target: PaletteTarget::Session(a.id.clone()),
                        text: format!("{at}{}/{}/{}", p.name, w.branch, a.name),
                        archived: a.archived,
                        status: Some(a.status),
                        unseen: a.unseen && !a.archived,
                        tier: session_tier(a),
                        // Archived rows order among themselves by when they
                        // were archived, the Sessions panel's rule.
                        interacted: if a.archived {
                            a.archived_at
                        } else {
                            last_interaction_ms(a, now)
                        },
                    });
                }
            }
        }
        // Pull requests go last so a query that also matches a session
        // still lands on the session first — the panels are what `/` is
        // mostly for. Only projects whose list has actually been fetched
        // contribute; the rest simply have nothing to offer yet.
        for p in &projects {
            let Some(open) = open_prs.get(&p.id) else {
                continue;
            };
            for pr in &open.list {
                items.push(PaletteItem {
                    target: PaletteTarget::PullRequest(pr.url.clone()),
                    text: format!("{at}{}/{}", p.name, pr.label()),
                    archived: false,
                    status: None,
                    unseen: false,
                    tier: PaletteTier::Rest,
                    interacted: 0,
                });
            }
        }
    }
    items
}

/// The workspaces `/` walks, in build order: the open one first (so among
/// never-run rows what's on screen wins), then the rest in tree order,
/// then any workspace only a project still refers to — see the orphan note
/// in [`build_palette_items`].
fn palette_workspace_order(tree: &Tree) -> Vec<WorkspaceId> {
    let mut order = vec![tree.active_workspace.clone()];
    let ids = tree
        .workspaces
        .iter()
        .map(|w| w.id.clone())
        .chain(tree.projects.iter().map(|p| p.workspace_id.clone()));
    for id in ids {
        if !order.contains(&id) {
            order.push(id);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{AgentKind, Workspace, Worktree};

    fn agent(id: &str, wt: &str, status: AgentStatus, unseen: bool, stamp: i64) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(wt.into()),
            name: id.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen,
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: stamp,
            alive: true,
            cloud_mirroring: false,
        }
    }

    fn project(ws: &str, id: &str, name: &str) -> Project {
        Project {
            workspace_id: WorkspaceId(ws.into()),
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: format!("/tmp/{name}").into(),
            sort_order: 0,
        }
    }

    fn worktree(id: &str, project: &str, branch: &str) -> Worktree {
        Worktree {
            id: WorktreeId(id.into()),
            project_id: ProjectId(project.into()),
            path: format!("/tmp/{branch}").into(),
            branch: branch.into(),
            is_main: branch == "main",
            sort_order: 0,
        }
    }

    /// Two workspaces: `default/demo` with `main` (a session waiting on
    /// you, one mid-turn, one never run) and `feat` (one unread finish,
    /// one read finish, newer), and `other/quiet`, which has never run.
    fn tree() -> Tree {
        Tree {
            workspaces: vec![
                Workspace {
                    id: WorkspaceId("default".into()),
                    name: "default".into(),
                },
                Workspace {
                    id: WorkspaceId("other".into()),
                    name: "other".into(),
                },
            ],
            active_workspace: WorkspaceId("default".into()),
            projects: vec![
                project("default", "p1", "demo"),
                project("other", "p2", "quiet"),
            ],
            worktrees: vec![
                worktree("w1", "p1", "main"),
                worktree("w2", "p1", "feat"),
                worktree("w3", "p2", "main"),
            ],
            agents: vec![
                agent("fresh", "w1", AgentStatus::Fresh, false, 0),
                agent("ask", "w1", AgentStatus::NeedsFeedback, false, 10),
                agent("run", "w1", AgentStatus::Running, false, 20),
                agent("read", "w2", AgentStatus::Finished, false, 1_000),
                agent("unread", "w2", AgentStatus::Finished, true, 500),
            ],
            terminals: Vec::new(),
            links: Vec::new(),
        }
    }

    fn rows(tree: &Tree, show_archived: bool, query: &str) -> Vec<String> {
        let mut palette = Palette::new(tree, show_archived, false, &HashMap::new());
        palette.query = TextInput::from(query);
        palette.apply_filter();
        palette
            .matches
            .iter()
            .map(|m| palette.items[m.item].text.clone())
            .collect()
    }

    #[test]
    fn empty_query_lists_needs_feedback_running_unread_then_everything_by_recency() {
        assert_eq!(
            rows(&tree(), false, ""),
            [
                // The attention tiers, whatever their stamps say.
                "default/demo/main/ask",
                "default/demo/main/run",
                "default/demo/feat/unread",
                // Then RECENCY ORDER: the rows over a working session count
                // as now, a stamp tie keeps workspace > project > worktree
                // > session…
                "default",
                "default/demo",
                "default/demo/main",
                "default/demo/feat",
                "default/demo/feat/read",
                // …and never-run rows keep build order at the bottom.
                "default/demo/main/fresh",
                "other",
                "other/quiet",
                "other/quiet/main",
            ]
        );
    }

    #[test]
    fn the_most_recent_worktree_leads_the_rest_tier() {
        let mut tree = tree();
        // Nothing needs attention: pure recency, feat (1000) over main (20).
        tree.agents
            .retain(|a| a.name == "read" || a.name == "fresh");
        tree.agents
            .push(agent("older", "w1", AgentStatus::Finished, false, 20));
        assert_eq!(
            rows(&tree, false, ""),
            [
                "default",
                "default/demo",
                "default/demo/feat",
                "default/demo/feat/read",
                "default/demo/main",
                "default/demo/main/older",
                "default/demo/main/fresh",
                "other",
                "other/quiet",
                "other/quiet/main",
            ]
        );
    }

    #[test]
    fn a_query_ranks_score_first_and_attention_on_ties() {
        let tree = tree();
        // Every `demo` row scores the same boundary run: attention decides.
        let demo = rows(&tree, false, "demo");
        assert_eq!(demo[0], "default/demo/main/ask");
        assert_eq!(demo[1], "default/demo/main/run");
        // A better match still beats a better tier: `read` starts a segment
        // in `feat/read`, sits mid-word in `feat/unread`.
        assert_eq!(
            rows(&tree, false, "read"),
            ["default/demo/feat/read", "default/demo/feat/unread"]
        );
    }

    #[test]
    fn archived_rows_sink_below_the_never_run_ones() {
        let mut tree = tree();
        let mut gone = agent("gone", "w1", AgentStatus::NeedsFeedback, false, 9_000);
        gone.archived = true;
        gone.archived_at = 9_000;
        tree.agents.push(gone);
        let listed = rows(&tree, true, "");
        assert_eq!(
            listed.last().map(String::as_str),
            Some("default/demo/main/gone")
        );
        assert!(!rows(&tree, false, "").iter().any(|t| t.ends_with("/gone")));
    }
}
