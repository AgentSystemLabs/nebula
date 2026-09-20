//! The `/` PALETTE: one fuzzy search over every WORKSPACE, PROJECT,
//! WORKTREE, SESSION and open pull request nebula knows about, in *every*
//! workspace — the "jump to anything" tool. Its rows are built here from
//! the tree; `event_loop.rs` handles the keys and the jump, `ui.rs` draws
//! the rows.
//!
//! The list is NESTED: each project is a header row with its rows indented
//! under it, so the palette reads project → session title instead of a
//! column of `workspace/project/branch/session` paths. Before a query it is
//! the RECENT OVERVIEW — only projects and their sessions, project by
//! project; typing searches everything, still grouped under the projects.

use crate::app::{
    clamp_selection, last_interaction_ms, now_ms, project_recency, project_rollup, project_unseen,
    window_start, workspace_recency, workspace_rollup, workspace_unseen, worktree_recency,
    worktree_rollup, worktree_unseen, OpenPrs, Tree,
};
use crate::pull_request::{Standing, Trouble};
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
    /// An open pull request on `project`'s repo, addressed by URL — the
    /// only identity it has, since nothing about a PR is stored. Picking it
    /// lands the Worktrees cursor on its row in that project's OPEN PRS
    /// group, so the pane reads it; the project is what says which
    /// workspace to switch to and which group to unfold on the way.
    PullRequest {
        project: ProjectId,
        url: String,
    },
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

/// One searchable row of the `/` palette. `text` is the string the fuzzy
/// filter runs over: every row carries its full path from the workspace
/// down — `workspace` for workspaces, `workspace/project` for projects,
/// `workspace/project/branch` for worktrees, `workspace/project/branch/name`
/// for sessions — so a query can narrow by any ancestor, a workspace name
/// included. Only the part from `label_at` on is drawn; the project the
/// rest names is the header the row sits under.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub target: PaletteTarget,
    pub text: String,
    /// Char index into `text` where the row's own name starts — past the
    /// path a query can still narrow by. The row draws only this part, and
    /// match positions before it light nothing.
    pub label_at: usize,
    /// Index into the palette's `items` of the project row this one nests
    /// under — every worktree, session and pull request row has one — or
    /// `None` for a project or workspace row, which sits at the top level.
    pub parent: Option<usize>,
    /// The raw stamp the row's dim "23m ago" reads, the one its panel row
    /// shows: `status_changed_at` for a session, the newest one under it
    /// for a worktree. 0 = never run, no label.
    pub stamped: i64,
    /// A project row outside the open workspace carries that workspace's
    /// name, drawn dim at the row's right edge, so two workspaces' projects of
    /// the same name are told apart and a pick that switches workspace
    /// says so first. `None` for every other row.
    pub workspace: Option<String>,
    pub archived: bool,
    /// The status this row's panel row would show: a rollup for projects
    /// and worktrees, its own status for a session. Drives the glyph color
    /// and the text sweep, so a running session reads as running in the
    /// palette too. Refreshed by [`Palette::rebuild`] as upserts land.
    pub status: Option<AgentStatus>,
    /// Whether anything under this row finished a turn nobody has read.
    /// Splits a finished dot green (read) from blue (waiting on you),
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
    /// Where a pull request row's PR stands — `Draft`, or `Open` for one
    /// that is ready for review; every row here is open, so those are the
    /// two it can be — and `None` for every other kind of row. The row
    /// spells it out after the title and takes its colors from it, so a
    /// draft is told from a finished pull request before it is picked, by
    /// the word and not only by the dim. Re-read by [`Palette::rebuild`]
    /// as list answers land, so a draft marked ready flips on the next
    /// refresh.
    pub standing: Option<Standing>,
    /// What a pull request row's PR is in trouble for — conflicts, or a
    /// failing check — and `None` for a healthy one and for every other
    /// kind of row. The row goes red for it and its badge names it
    /// (`merge conflicts`, `checks failing`) in place of the standing.
    pub trouble: Option<Trouble>,
}

/// One visible palette row: an index into `items` plus the char positions of
/// `text` the query matched, for highlighting.
#[derive(Debug, Clone)]
pub struct PaletteMatch {
    pub item: usize,
    pub positions: Vec<usize>,
    /// Drawn indented under the project header above it.
    pub nested: bool,
    /// A project header the query did not match itself, listed only to
    /// place the rows under it that did. Still a row the cursor can pick;
    /// it just does not count toward the title's `(hits/total)`.
    pub context: bool,
}

/// Fuzzy-search palette over every workspace, project, worktree, and
/// session (`/`), across all workspaces — not just the open one.
#[derive(Debug, Clone)]
pub struct Palette {
    pub items: Vec<PaletteItem>,
    /// Type-to-filter query over `items` texts; always live.
    pub query: TextInput,
    /// Visible rows, NESTED: `items` narrowed by `query` (to projects and
    /// sessions while it is empty), best matches first — ties, and the
    /// whole list before a query, in the attention order of
    /// [`PaletteTier`] then most recent interaction — then folded under
    /// their projects: each project where its best row ranks, its rows
    /// after it in that same order. See [`nest`].
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
        hide_draft_prs: bool,
    ) -> Self {
        let mut palette = Self {
            items: build_palette_items(tree, show_archived, open_prs, hide_draft_prs),
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
        hide_draft_prs: bool,
    ) {
        let keep = self.selected_target().cloned();
        self.items = build_palette_items(tree, show_archived, open_prs, hide_draft_prs);
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

    /// Recompute `matches` from `query` and put the selection on the best
    /// row. Best matches first; the attention order breaks ties and is the
    /// whole order when the query is empty, so `/` `Enter` lands on the
    /// session that needs you before anything else — nested under its
    /// project, which is why the cursor goes to that row and not to the
    /// header drawn above it.
    pub fn apply_filter(&mut self) {
        let rank = attention_rank(&self.items);
        let overview = self.query.trim().is_empty();
        let ranked: Vec<(usize, Vec<usize>)> = crate::fuzzy::rank_by(
            &self.query,
            self.items.iter().map(|i| i.text.as_str()),
            |i, _| rank[i],
        )
        .into_iter()
        .filter(|(i, _)| !overview || self.items[*i].in_overview())
        .collect();
        // Before a query the cursor starts on a session: a project ties its
        // newest session's stamp and wins the tie on build order, but its
        // header is drawn right above that session anyway, and `/` `Enter`
        // is for going back to what you ran. A query keeps its best match,
        // project or not.
        let best = ranked
            .iter()
            .find(|(i, _)| !overview || matches!(self.items[*i].target, PaletteTarget::Session(_)))
            .or(ranked.first())
            .map(|(i, _)| *i);
        self.matches = nest(&self.items, ranked);
        self.selected = best
            .and_then(|b| self.matches.iter().position(|m| m.item == b))
            .unwrap_or(0);
    }

    /// Rows the query actually matched — the title's count, which leaves
    /// out the context headers placed only to hold them.
    pub fn hits(&self) -> usize {
        self.matches.iter().filter(|m| !m.context).count()
    }
}

impl PaletteItem {
    /// Whether the row belongs in the RECENT OVERVIEW `/` opens on before
    /// anything is typed: the projects and the sessions under them. The
    /// workspaces, worktrees and pull requests wait for a query.
    fn in_overview(&self) -> bool {
        matches!(
            self.target,
            PaletteTarget::Project(_) | PaletteTarget::Session(_)
        )
    }
}

/// Fold a best-first ranking into the NESTED list: each project header
/// where its best-ranked row (or itself) first appears, then that
/// project's rows in rank order, indented; a workspace row stands alone at
/// the top level. A project the query did not match comes along as a
/// `context` header so its rows never float free of it.
fn nest(items: &[PaletteItem], ranked: Vec<(usize, Vec<usize>)>) -> Vec<PaletteMatch> {
    struct Group {
        head: usize,
        /// The header's own match, `None` while it is only context.
        head_positions: Option<Vec<usize>>,
        rows: Vec<(usize, Vec<usize>)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for (i, positions) in ranked {
        let head = items[i].parent.unwrap_or(i);
        let g = *slot.entry(head).or_insert_with(|| {
            groups.push(Group {
                head,
                head_positions: None,
                rows: Vec::new(),
            });
            groups.len() - 1
        });
        if head == i {
            groups[g].head_positions = Some(positions);
        } else {
            groups[g].rows.push((i, positions));
        }
    }
    let mut matches = Vec::new();
    for g in groups {
        matches.push(PaletteMatch {
            item: g.head,
            context: g.head_positions.is_none(),
            positions: g.head_positions.unwrap_or_default(),
            nested: false,
        });
        matches.extend(g.rows.into_iter().map(|(item, positions)| PaletteMatch {
            item,
            positions,
            nested: true,
            context: false,
        }));
    }
    matches
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

/// The SESSION rows of the `/` palette in its attention order — what `]`
/// and `[` step through with no modal open: the same [`attention_rank`]
/// the palette applies before a query is typed, kept to the rows that are
/// sessions. Every workspace contributes, not only the open one, and
/// archived sessions are left out whatever the SESSIONS PANEL's toggle
/// says: a released PTY has nothing left to ask of anyone.
pub fn attention_sessions(tree: &Tree) -> Vec<AgentId> {
    let items = build_palette_items(tree, false, &HashMap::new(), false);
    let rank = attention_rank(&items);
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| rank[i]);
    order
        .into_iter()
        .filter_map(|i| match &items[i].target {
            PaletteTarget::Session(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
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
/// (the Sessions panel rule); draft pull requests only while
/// `hide_draft_prs` is off (the PROJECT OPEN PRS GROUP's rule — the two
/// surfaces show the same rows). Worktrees and sessions are never held
/// back by either.
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
    hide_draft_prs: bool,
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
                label_at: 0,
                parent: None,
                stamped: workspace_recency(tree, &ws.id, now).stamped,
                workspace: None,
                archived: false,
                status: workspace_rollup(tree, &ws.id),
                unseen: workspace_unseen(tree, &ws.id) > 0,
                tier: PaletteTier::Rest,
                interacted: workspace_recency(tree, &ws.id, now).interacted,
                standing: None,
                trouble: None,
            });
        }
        let at = match workspace {
            Some(ws) => format!("{}/", ws.name),
            None => String::new(),
        };
        // Named on each of its project headers when it isn't the one open.
        let away = workspace
            .filter(|ws| ws.id != tree.active_workspace)
            .map(|ws| ws.name.clone());
        let projects: Vec<&Project> = tree
            .projects
            .iter()
            .filter(|p| p.workspace_id == id)
            .collect();
        // Project `k`'s row is `first + k`: the header its rows nest under.
        let first = items.len();
        // Within a workspace the kinds stay grouped project → worktree →
        // session, so a bare query still ranks the shallowest match first.
        for p in &projects {
            items.push(PaletteItem {
                target: PaletteTarget::Project(p.id.clone()),
                text: format!("{at}{}", p.name),
                label_at: at.chars().count(),
                parent: None,
                stamped: project_recency(tree, &p.id, now).stamped,
                workspace: away.clone(),
                archived: false,
                status: project_rollup(tree, &p.id),
                unseen: project_unseen(tree, &p.id) > 0,
                tier: PaletteTier::Rest,
                interacted: project_recency(tree, &p.id, now).interacted,
                standing: None,
                trouble: None,
            });
        }
        for (k, p) in projects.iter().enumerate() {
            let under = format!("{at}{}/", p.name);
            for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
                items.push(PaletteItem {
                    target: PaletteTarget::Worktree(w.id.clone()),
                    text: format!("{under}{}", w.branch),
                    label_at: under.chars().count(),
                    parent: Some(first + k),
                    stamped: worktree_recency(tree, &w.id, now).stamped,
                    workspace: None,
                    archived: false,
                    status: worktree_rollup(tree, &w.id),
                    unseen: worktree_unseen(tree, &w.id) > 0,
                    tier: PaletteTier::Rest,
                    interacted: worktree_recency(tree, &w.id, now).interacted,
                    standing: None,
                    trouble: None,
                });
            }
        }
        for (k, p) in projects.iter().enumerate() {
            for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
                // The branch stays in the searched path but not the drawn
                // label: the row reads project → session title.
                let under = format!("{at}{}/{}/", p.name, w.branch);
                for a in tree.agents.iter().filter(|a| a.worktree_id == w.id) {
                    if a.archived && !show_archived {
                        continue;
                    }
                    items.push(PaletteItem {
                        target: PaletteTarget::Session(a.id.clone()),
                        text: format!("{under}{}", a.name),
                        label_at: under.chars().count(),
                        parent: Some(first + k),
                        stamped: a.status_changed_at,
                        workspace: None,
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
                        standing: None,
                        trouble: None,
                    });
                }
            }
        }
        // Pull requests go last so a query that also matches a session
        // still lands on the session first — the panels are what `/` is
        // mostly for. Only projects whose list has actually been fetched
        // contribute; the rest simply have nothing to offer yet.
        for (k, p) in projects.iter().enumerate() {
            let Some(open) = open_prs.get(&p.id) else {
                continue;
            };
            let under = format!("{at}{}/", p.name);
            for pr in &open.list {
                if hide_draft_prs && pr.is_draft {
                    continue;
                }
                items.push(PaletteItem {
                    target: PaletteTarget::PullRequest {
                        project: p.id.clone(),
                        url: pr.url.clone(),
                    },
                    text: format!("{under}{}", pr.label()),
                    label_at: under.chars().count(),
                    parent: Some(first + k),
                    stamped: 0,
                    workspace: None,
                    archived: false,
                    status: None,
                    unseen: false,
                    tier: PaletteTier::Rest,
                    interacted: 0,
                    standing: Some(pr.standing()),
                    trouble: pr.trouble(),
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
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: stamp,
            alive: true,
            recent_prompts: Vec::new(),
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

    /// The palette as drawn: a project header by its label (with the other
    /// workspace it lives in, `◇ other`), the rows under it stepped in two
    /// columns, and the row the cursor starts on marked `▶`.
    fn rows(tree: &Tree, show_archived: bool, query: &str) -> Vec<String> {
        let mut palette = Palette::new(tree, show_archived, false, &HashMap::new(), false);
        palette.query = TextInput::from(query);
        palette.apply_filter();
        palette
            .matches
            .iter()
            .enumerate()
            .map(|(row, m)| {
                let item = &palette.items[m.item];
                let label: String = item.text.chars().skip(item.label_at).collect();
                let away = item
                    .workspace
                    .as_ref()
                    .map_or(String::new(), |ws| format!(" ◇ {ws}"));
                let mark = if row == palette.selected { "▶" } else { "" };
                let indent = if m.nested { "  " } else { "" };
                format!("{mark}{indent}{label}{away}")
            })
            .collect()
    }

    /// Before a query `/` is the RECENT OVERVIEW: each project with its
    /// sessions under it — no workspace, worktree or branch in the way —
    /// the project that needs you first, and inside it the attention
    /// order: NEEDS FEEDBACK, RUNNING, UNSEEN, then by last interaction,
    /// never-run at the bottom. The cursor starts on the session that
    /// needs you, not on the header drawn above it.
    #[test]
    fn empty_query_nests_sessions_under_projects_in_attention_order() {
        assert_eq!(
            rows(&tree(), false, ""),
            [
                "demo",
                "▶  ask",
                "  run",
                "  unread",
                "  read",
                "  fresh",
                // The other workspace's project, named as such.
                "quiet ◇ other",
            ]
        );
    }

    /// Nothing needs attention: projects and their sessions in RECENCY
    /// ORDER, and the cursor on the most recent session — `/` `Enter` goes
    /// back to what you last ran, not to its project.
    #[test]
    fn with_nothing_waiting_the_most_recent_session_leads() {
        let mut tree = tree();
        tree.agents
            .retain(|a| a.name == "read" || a.name == "fresh");
        tree.agents
            .push(agent("older", "w1", AgentStatus::Finished, false, 20));
        assert_eq!(
            rows(&tree, false, ""),
            ["demo", "▶  read", "  older", "  fresh", "quiet ◇ other"]
        );
    }

    /// The overview leaves out the workspace, worktree and pull request
    /// rows; a query reaches them, still grouped: a workspace row stands
    /// alone at the top level, a worktree nests under its project.
    #[test]
    fn a_query_reaches_the_rows_the_overview_leaves_out() {
        let tree = tree();
        let palette = Palette::new(&tree, false, false, &HashMap::new(), false);
        assert!(
            palette.matches.iter().all(|m| matches!(
                palette.items[m.item].target,
                PaletteTarget::Project(_) | PaletteTarget::Session(_)
            )),
            "only projects and sessions before a query"
        );
        assert_eq!(
            rows(&tree, false, "other"),
            ["▶other", "quiet ◇ other", "  main"]
        );
    }

    #[test]
    fn a_query_ranks_score_first_and_attention_on_ties() {
        let tree = tree();
        // Every `demo` row scores the same boundary run: attention decides,
        // and the header — which matched too — leads its rows.
        let demo = rows(&tree, false, "demo");
        assert_eq!(demo[..3], ["demo", "▶  ask", "  run"]);
        // A better match still beats a better tier: `read` starts a segment
        // in `feat/read`, sits mid-word in `feat/unread`. Their project
        // matched nothing, but still heads them.
        assert_eq!(rows(&tree, false, "read"), ["demo", "▶  read", "  unread"]);
    }

    /// A header the query only brought along to hold its rows is context:
    /// pickable, but not a hit — `(2/12)` counts the two sessions.
    #[test]
    fn context_headers_do_not_count_as_hits() {
        let tree = tree();
        let mut palette = Palette::new(&tree, false, false, &HashMap::new(), false);
        palette.query = TextInput::from("read");
        palette.apply_filter();
        assert_eq!(palette.matches.len(), 3);
        assert!(palette.matches[0].context);
        assert!(palette.matches[0].positions.is_empty());
        assert_eq!(palette.hits(), 2);
    }

    /// The `]` / `[` ring is the palette's session rows in the palette's
    /// order — attention tiers, then recency, never-run last — with the
    /// workspaces, projects, worktrees and archived sessions left out.
    #[test]
    fn attention_sessions_is_the_palette_order_kept_to_live_sessions() {
        let mut tree = tree();
        let mut gone = agent("gone", "w1", AgentStatus::NeedsFeedback, false, 9_000);
        gone.archived = true;
        gone.archived_at = 9_000;
        tree.agents.push(gone);
        let ring: Vec<String> = attention_sessions(&tree)
            .into_iter()
            .map(|id| id.0)
            .collect();
        assert_eq!(ring, ["ask", "run", "unread", "read", "fresh"]);
    }

    /// `hide_draft_prs` keeps drafts out of `/` exactly as it keeps them
    /// out of the PROJECT OPEN PRS GROUP: the finished pull request is
    /// still a row, the draft is not, and nothing else on the project is
    /// touched — its worktrees and sessions are what the toggle promises
    /// to leave alone.
    #[test]
    fn hidden_drafts_leave_the_palette_and_nothing_else_does() {
        let tree = tree();
        let pr = |number: u64, title: &str, is_draft: bool| crate::pull_request::OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft,
            health: Default::default(),
            head: format!("pr-{number}"),
        };
        let now = std::time::Instant::now();
        let mut open_prs = HashMap::new();
        open_prs.insert(
            ProjectId("p1".into()),
            OpenPrs {
                list: vec![pr(7, "Attach links", false), pr(9, "Still cooking", true)],
                at: now,
                due: now,
                step: std::time::Duration::from_secs(1),
            },
        );
        let texts = |hide: bool| -> Vec<String> {
            Palette::new(&tree, false, false, &open_prs, hide)
                .items
                .iter()
                .map(|i| i.text.clone())
                .collect()
        };

        let shown = texts(false);
        assert!(
            shown.iter().any(|t| t == "default/demo/#7 Attach links"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|t| t == "default/demo/#9 Still cooking"),
            "{shown:?}"
        );

        let hidden = texts(true);
        assert!(
            hidden.iter().any(|t| t == "default/demo/#7 Attach links"),
            "{hidden:?}"
        );
        assert!(!hidden.iter().any(|t| t.contains("#9")), "{hidden:?}");
        assert_eq!(hidden.len(), shown.len() - 1, "only the draft row went");
        assert!(
            hidden.iter().any(|t| t == "default/demo/main/ask"),
            "sessions are untouched: {hidden:?}"
        );
    }

    /// A pull request row carries what its PR is in trouble for, so `/`
    /// paints it red and names it — `merge conflicts`, `checks failing` —
    /// the way the sidebar's row does; a healthy one, and every other
    /// kind of row, carries none.
    #[test]
    fn pull_request_rows_carry_their_trouble() {
        use crate::app::OpenPrs;
        use crate::pull_request::{Checks, Health, OpenPr};
        let tree = tree();
        let pr = |number: u64, health: Health| OpenPr {
            number,
            title: format!("pr {number}"),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft: false,
            health,
            head: format!("pr-{number}"),
        };
        let now = std::time::Instant::now();
        let mut open_prs = HashMap::new();
        open_prs.insert(
            ProjectId("p1".into()),
            OpenPrs {
                list: vec![
                    pr(7, Health::default()),
                    pr(
                        8,
                        Health {
                            conflicts: true,
                            checks: Checks::Passing,
                        },
                    ),
                    pr(
                        9,
                        Health {
                            conflicts: false,
                            checks: Checks::Failing,
                        },
                    ),
                ],
                at: now,
                due: now,
                step: std::time::Duration::from_secs(1),
            },
        );
        let palette = Palette::new(&tree, false, false, &open_prs, false);
        let troubles: Vec<(&str, Option<Trouble>)> = palette
            .items
            .iter()
            .filter(|i| matches!(i.target, PaletteTarget::PullRequest { .. }))
            .map(|i| (i.text.as_str(), i.trouble))
            .collect();
        assert_eq!(
            troubles,
            [
                ("default/demo/#7 pr 7", None),
                ("default/demo/#8 pr 8", Some(Trouble::Conflicts)),
                ("default/demo/#9 pr 9", Some(Trouble::FailingChecks)),
            ]
        );
        assert!(
            palette
                .items
                .iter()
                .filter(|i| !matches!(i.target, PaletteTarget::PullRequest { .. }))
                .all(|i| i.trouble.is_none()),
            "no other row is in trouble"
        );
    }

    /// A pull request row knows whether its PR is a draft or ready for
    /// review — the one thing the row can say about it beyond the title —
    /// and no other kind of row carries a standing at all. The list's
    /// order (drafts sunk last) is the panel's, applied where the answer
    /// lands, so here the rows come in the order the list holds them.
    #[test]
    fn pull_request_rows_carry_their_standing_and_nothing_else_does() {
        use crate::app::OpenPrs;
        use crate::pull_request::OpenPr;
        let tree = tree();
        let pr = |number: u64, title: &str, is_draft: bool| OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft,
            health: Default::default(),
            head: format!("pr-{number}"),
        };
        let now = std::time::Instant::now();
        let mut open_prs = HashMap::new();
        open_prs.insert(
            ProjectId("p1".into()),
            OpenPrs {
                list: vec![
                    pr(7, "Attach links", false),
                    pr(9, "Number the lines", true),
                ],
                at: now,
                due: now,
                step: std::time::Duration::from_secs(1),
            },
        );
        let palette = Palette::new(&tree, false, false, &open_prs, false);
        let standings: Vec<(&str, Option<Standing>)> = palette
            .items
            .iter()
            .filter(|i| matches!(i.target, PaletteTarget::PullRequest { .. }))
            .map(|i| (i.text.as_str(), i.standing))
            .collect();
        assert_eq!(
            standings,
            [
                ("default/demo/#7 Attach links", Some(Standing::Open)),
                ("default/demo/#9 Number the lines", Some(Standing::Draft)),
            ]
        );
        assert!(
            palette
                .items
                .iter()
                .filter(|i| !matches!(i.target, PaletteTarget::PullRequest { .. }))
                .all(|i| i.standing.is_none()),
            "only a pull request has a standing"
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
            listed[listed.len() - 2..],
            ["  gone", "quiet ◇ other"],
            "last under its project: {listed:?}"
        );
        assert!(!rows(&tree, false, "").iter().any(|t| t.ends_with("gone")));
    }
}
