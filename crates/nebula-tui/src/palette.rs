//! The `/` PALETTE: one fuzzy search over every PROJECT, WORKTREE,
//! SESSION and open pull request nebula knows about — the "jump to
//! anything" tool. Its rows are built here from
//! the tree; `event_loop.rs` handles the keys and the jump, `ui.rs` draws
//! the rows.
//!
//! The list is FLAT: one row per thing, each drawing the project it lives
//! in dim before its own name — `demo/fix-login` — instead of a header row
//! with its rows indented under it, and instead of a column of full
//! `project/branch/session` paths. Before a query it is the RECENT
//! SESSIONS list: only the sessions, the ones that want you first; typing
//! reaches the projects, worktrees and pull requests too, on lines of
//! their own in that same order.

use crate::app::{
    clamp_selection, last_interaction_ms, now_ms, project_recency, project_rollup, project_unseen,
    window_start, worktree_recency, worktree_rollup, worktree_unseen, OpenPrs, Tree,
};
use crate::pull_request::{Standing, Trouble};
use crate::text_input::TextInput;
use nebula_core::{Agent, AgentId, AgentStatus, Project, ProjectId, WorktreeId};
use ratatui::layout::Rect;
use std::collections::HashMap;

/// What a `/` palette row jumps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteTarget {
    Project(ProjectId),
    Worktree(WorktreeId),
    Session(AgentId),
    /// An open pull request on `project`'s repo, addressed by URL — the
    /// only identity it has, since nothing about a PR is stored. Picking it
    /// lands the Worktrees cursor on its row in that project's OPEN PRS
    /// group, so the pane reads it; the project is what says which group
    /// to unfold on the way.
    PullRequest {
        project: ProjectId,
        url: String,
    },
}

/// Where a `/` row sits before the query has said anything — the tiers of
/// the PALETTE's attention order, best first. A SESSION waiting on you
/// (NEEDS FEEDBACK) comes first, then one mid-turn (RUNNING), then one that
/// finished a turn nobody has read (UNSEEN); every other row — read and
/// never-run sessions, and every project, worktree and pull request — sorts under those in RECENCY ORDER, so the checkout you were
/// just in is the first thing after what needs you. ARCHIVED sessions have
/// no tier because they have no row — see [`build_palette_items`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteTier {
    NeedsFeedback,
    Running,
    Unseen,
    Rest,
}

/// One searchable row of the `/` palette. `text` is the string the fuzzy
/// filter runs over: every row carries its full path from the project
/// down — `project` for projects, `project/branch` for worktrees,
/// `project/branch/name` for sessions — so a query can narrow by any
/// ancestor. Two slices of it are drawn: the `crumb` (the project) and
/// everything from `label_at` on (the row's own name). What lies between
/// — a session's branch — is searched but not drawn.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub target: PaletteTarget,
    pub text: String,
    /// Char index into `text` where the row's own name starts — past the
    /// path a query can still narrow by. The row draws this part and the
    /// `crumb`; match positions outside both light nothing.
    pub label_at: usize,
    /// Char range in `text` of the CRUMB drawn dim before the row's own
    /// name — the project it lives in, which every worktree, session and
    /// pull request row carries, so a flat row still says where it is
    /// (`demo/fix-login`). `None` on a project row, which is its own name
    /// already.
    pub crumb: Option<(usize, usize)>,
    /// The raw stamp the row's dim "23m ago" reads, the one its panel row
    /// shows: `status_changed_at` for a session, the newest one under it
    /// for a worktree. 0 = never run, no label.
    pub stamped: i64,
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
    /// project or worktree, nothing for a pull request. 0 sorts
    /// last within its tier.
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
}

/// Fuzzy-search palette over every project, worktree, and session (`/`).
#[derive(Debug, Clone)]
pub struct Palette {
    pub items: Vec<PaletteItem>,
    /// Type-to-filter query over `items` texts; always live.
    pub query: TextInput,
    /// Visible rows, FLAT: `items` narrowed by `query` (to the sessions
    /// while it is empty), best matches first — ties, and the whole list
    /// before a query, in the attention order of [`PaletteTier`] then most
    /// recent interaction. One row per thing, nothing folded under
    /// anything.
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
        enter_attaches: bool,
        open_prs: &HashMap<ProjectId, OpenPrs>,
        hide_draft_prs: bool,
    ) -> Self {
        let mut palette = Self {
            items: build_palette_items(tree, open_prs, hide_draft_prs),
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
        open_prs: &HashMap<ProjectId, OpenPrs>,
        hide_draft_prs: bool,
    ) {
        let keep = self.selected_target().cloned();
        self.items = build_palette_items(tree, open_prs, hide_draft_prs);
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
    /// row — the top one, since the list is flat and nothing is drawn only
    /// to hold something else. Best matches first; the attention order
    /// breaks ties and is the whole order when the query is empty, so `/`
    /// `Enter` lands on the session that needs you before anything else.
    pub fn apply_filter(&mut self) {
        let rank = attention_rank(&self.items);
        let overview = self.query.trim().is_empty();
        self.matches = crate::fuzzy::rank_by(
            &self.query,
            self.items.iter().map(|i| i.text.as_str()),
            |i, _| rank[i],
        )
        .into_iter()
        .filter(|(i, _)| !overview || self.items[*i].in_overview())
        .map(|(item, positions)| PaletteMatch { item, positions })
        .collect();
        self.selected = 0;
    }

    /// Rows the query matched — the title's count, which is every visible
    /// row now that the list is flat.
    pub fn hits(&self) -> usize {
        self.matches.len()
    }
}

impl PaletteItem {
    /// Whether the row belongs in the RECENT SESSIONS list `/` opens on
    /// before anything is typed: the sessions, and only those. The
    /// projects, worktrees and pull requests wait for a query.
    fn in_overview(&self) -> bool {
        matches!(self.target, PaletteTarget::Session(_))
    }
}

/// Each item's position in the attention order: tier first, then most
/// recently interacted, then build order — which keeps a project over its
/// worktrees over their sessions when they share a stamp, and never-run
/// rows in tree order.
fn attention_rank(items: &[PaletteItem]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (items[i].tier, std::cmp::Reverse(items[i].interacted), i));
    let mut rank = vec![0; items.len()];
    for (pos, i) in order.into_iter().enumerate() {
        rank[i] = pos;
    }
    rank
}

/// The SESSION rows of the `/` palette in its attention order — what `.`
/// and `,` step through with no modal open: the same [`attention_rank`]
/// the palette applies before a query is typed, kept to the rows that are
/// sessions. Every project contributes, not only the one on screen, and
/// archived sessions are left out, here as in the palette itself: a
/// released PTY has nothing left to ask of anyone.
pub fn attention_sessions(tree: &Tree) -> Vec<AgentId> {
    let items = build_palette_items(tree, &HashMap::new(), false);
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
/// UNSEEN flag the DONE BADGE counts. Archived sessions never reach here:
/// they are not rows at all.
fn session_tier(a: &Agent) -> PaletteTier {
    match a.status {
        AgentStatus::NeedsFeedback => PaletteTier::NeedsFeedback,
        AgentStatus::Running => PaletteTier::Running,
        AgentStatus::Finished if a.unseen => PaletteTier::Unseen,
        _ => PaletteTier::Rest,
    }
}

/// Every jumpable entity: each project in tree order, then their
/// worktrees, then their sessions, then the open pull requests nebula has
/// fetched. ARCHIVED SESSIONS ARE NEVER ROWS, whatever the SESSIONS
/// PANEL's `A` toggle shows: a released PTY has nothing left to jump to,
/// and the find-anything tool is for what is still live. Draft pull
/// requests are left out only while `hide_draft_prs` is on (the PROJECT
/// OPEN PRS GROUP's rule — the two surfaces show the same rows);
/// worktrees are never held back.
///
/// This is the build order — what `matches` falls back to among rows with
/// the same tier and stamp. The order the user sees is
/// [`attention_rank`]'s.
fn build_palette_items(
    tree: &Tree,
    open_prs: &HashMap<ProjectId, OpenPrs>,
    hide_draft_prs: bool,
) -> Vec<PaletteItem> {
    let now = now_ms();
    let mut items = Vec::new();
    // Where each project's own name sits inside its rows' paths: the
    // crumb every row under it draws dim before its own name.
    let crumb = |p: &Project| Some((0, p.name.chars().count()));
    let projects: Vec<&Project> = tree.projects.iter().collect();
    // The kinds stay grouped project → worktree → session, so a bare
    // query still ranks the shallowest match first.
    for p in &projects {
        items.push(PaletteItem {
            target: PaletteTarget::Project(p.id.clone()),
            text: p.name.clone(),
            label_at: 0,
            crumb: None,
            stamped: project_recency(tree, &p.id, now).stamped,
            status: project_rollup(tree, &p.id),
            unseen: project_unseen(tree, &p.id) > 0,
            tier: PaletteTier::Rest,
            interacted: project_recency(tree, &p.id, now).interacted,
            standing: None,
            trouble: None,
        });
    }
    for p in &projects {
        let under = format!("{}/", p.name);
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            items.push(PaletteItem {
                target: PaletteTarget::Worktree(w.id.clone()),
                text: format!("{under}{}", w.branch),
                label_at: under.chars().count(),
                crumb: crumb(p),
                stamped: worktree_recency(tree, &w.id, now).stamped,
                status: worktree_rollup(tree, &w.id),
                unseen: worktree_unseen(tree, &w.id) > 0,
                tier: PaletteTier::Rest,
                interacted: worktree_recency(tree, &w.id, now).interacted,
                standing: None,
                trouble: None,
            });
        }
    }
    for p in &projects {
        for w in tree.worktrees.iter().filter(|w| w.project_id == p.id) {
            // The branch stays in the searched path but not the drawn
            // label: the row reads project → session title.
            let under = format!("{}/{}/", p.name, w.branch);
            for a in tree.agents.iter().filter(|a| a.worktree_id == w.id) {
                // An ARCHIVED session is not a jump target: its PTY is
                // released, so there is nothing to attach to or answer.
                if a.archived {
                    continue;
                }
                items.push(PaletteItem {
                    target: PaletteTarget::Session(a.id.clone()),
                    text: format!("{under}{}", a.name),
                    label_at: under.chars().count(),
                    crumb: crumb(p),
                    stamped: a.status_changed_at,
                    status: Some(a.status),
                    unseen: a.unseen,
                    tier: session_tier(a),
                    interacted: last_interaction_ms(a, now),
                    standing: None,
                    trouble: None,
                });
            }
        }
    }
    // Pull requests go last so a query that also matches a session still
    // lands on the session first — the panels are what `/` is mostly for.
    // Only projects whose list has actually been fetched contribute; the
    // rest simply have nothing to offer yet.
    for p in &projects {
        let Some(open) = open_prs.get(&p.id) else {
            continue;
        };
        let under = format!("{}/", p.name);
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
                crumb: crumb(p),
                stamped: 0,
                status: None,
                unseen: false,
                tier: PaletteTier::Rest,
                interacted: 0,
                standing: Some(pr.standing()),
                trouble: pr.trouble(),
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::{AgentKind, Worktree};

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

    fn project(id: &str, name: &str) -> Project {
        Project {
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

    /// Two projects: `demo` with `main` (a session waiting on you, one
    /// mid-turn, one never run) and `feat` (one unread finish, one read
    /// finish, newer), and `quiet`, which has never run.
    fn tree() -> Tree {
        Tree {
            projects: vec![project("p1", "demo"), project("p2", "quiet")],
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

    /// The palette as drawn: every row on a line of its own, its project
    /// crumb then its own name (`demo/ask`), and the row the cursor starts
    /// on marked `▶`.
    fn rows(tree: &Tree, query: &str) -> Vec<String> {
        let mut palette = Palette::new(tree, false, &HashMap::new(), false);
        palette.query = TextInput::from(query);
        palette.apply_filter();
        palette
            .matches
            .iter()
            .enumerate()
            .map(|(row, m)| {
                let item = &palette.items[m.item];
                let crumb = item.crumb.map_or(String::new(), |(at, end)| {
                    let name: String = item.text.chars().take(end).skip(at).collect();
                    format!("{name}/")
                });
                let label: String = item.text.chars().skip(item.label_at).collect();
                let mark = if row == palette.selected { "▶" } else { "" };
                format!("{mark}{crumb}{label}")
            })
            .collect()
    }

    /// Before a query `/` is the RECENT SESSIONS list: one flat line per
    /// session, each naming the project it lives in — no worktree or
    /// branch in the way, and no header rows — in the
    /// attention order NEEDS FEEDBACK, RUNNING, UNSEEN, then by last
    /// interaction, never-run at the bottom. The cursor starts on the
    /// session that needs you. `quiet`, a project with no sessions, is not
    /// a row until something is typed.
    #[test]
    fn empty_query_lists_the_sessions_flat_in_attention_order() {
        assert_eq!(
            rows(&tree(), ""),
            [
                "▶demo/ask",
                "demo/run",
                "demo/unread",
                "demo/read",
                "demo/fresh",
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
        assert_eq!(rows(&tree, ""), ["▶demo/read", "demo/older", "demo/fresh"]);
    }

    /// The overview leaves out the project, worktree and pull request rows;
    /// a query reaches them, each on a line of its own — a project by its
    /// bare name, a worktree with its project in front.
    #[test]
    fn a_query_reaches_the_rows_the_overview_leaves_out() {
        let tree = tree();
        let palette = Palette::new(&tree, false, &HashMap::new(), false);
        assert!(
            palette
                .matches
                .iter()
                .all(|m| matches!(palette.items[m.item].target, PaletteTarget::Session(_))),
            "only sessions before a query"
        );
        assert_eq!(rows(&tree, "quiet"), ["▶quiet", "quiet/main"]);
    }

    #[test]
    fn a_query_ranks_score_first_and_attention_on_ties() {
        let tree = tree();
        // Every `demo` row scores the same boundary run, so attention alone
        // decides — the session waiting on you leads, the project row it
        // used to sit under is just another line further down.
        let demo = rows(&tree, "demo");
        assert_eq!(demo[..3], ["▶demo/ask", "demo/run", "demo/unread"]);
        // A better match still beats a better tier: `read` starts a segment
        // in `feat/read`, sits mid-word in `feat/unread`.
        assert_eq!(rows(&tree, "read"), ["▶demo/read", "demo/unread"]);
    }

    /// Nothing is listed only to hold something else any more, so every
    /// visible row is a hit: `(2/12)` counts exactly the rows drawn.
    #[test]
    fn every_visible_row_counts_as_a_hit() {
        let tree = tree();
        let mut palette = Palette::new(&tree, false, &HashMap::new(), false);
        palette.query = TextInput::from("read");
        palette.apply_filter();
        assert_eq!(palette.matches.len(), 2);
        assert!(palette.matches.iter().all(|m| !m.positions.is_empty()));
        assert_eq!(palette.hits(), 2);
    }

    /// The `.` / `,` ring is the palette's session rows in the palette's
    /// order — attention tiers, then recency, never-run last — with the
    /// projects, worktrees and archived sessions left out.
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
            Palette::new(&tree, false, &open_prs, hide)
                .items
                .iter()
                .map(|i| i.text.clone())
                .collect()
        };

        let shown = texts(false);
        assert!(
            shown.iter().any(|t| t == "demo/#7 Attach links"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|t| t == "demo/#9 Still cooking"),
            "{shown:?}"
        );

        let hidden = texts(true);
        assert!(
            hidden.iter().any(|t| t == "demo/#7 Attach links"),
            "{hidden:?}"
        );
        assert!(!hidden.iter().any(|t| t.contains("#9")), "{hidden:?}");
        assert_eq!(hidden.len(), shown.len() - 1, "only the draft row went");
        assert!(
            hidden.iter().any(|t| t == "demo/main/ask"),
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
        let palette = Palette::new(&tree, false, &open_prs, false);
        let troubles: Vec<(&str, Option<Trouble>)> = palette
            .items
            .iter()
            .filter(|i| matches!(i.target, PaletteTarget::PullRequest { .. }))
            .map(|i| (i.text.as_str(), i.trouble))
            .collect();
        assert_eq!(
            troubles,
            [
                ("demo/#7 pr 7", None),
                ("demo/#8 pr 8", Some(Trouble::Conflicts)),
                ("demo/#9 pr 9", Some(Trouble::FailingChecks)),
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
        let palette = Palette::new(&tree, false, &open_prs, false);
        let standings: Vec<(&str, Option<Standing>)> = palette
            .items
            .iter()
            .filter(|i| matches!(i.target, PaletteTarget::PullRequest { .. }))
            .map(|i| (i.text.as_str(), i.standing))
            .collect();
        assert_eq!(
            standings,
            [
                ("demo/#7 Attach links", Some(Standing::Open)),
                ("demo/#9 Number the lines", Some(Standing::Draft)),
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

    /// An ARCHIVED session is never a `/` row — not in the RECENT
    /// OVERVIEW, and not for a query that spells its name out. The
    /// SESSIONS PANEL's `A` toggle has no say here: it is the panel's
    /// fold, not the palette's filter.
    #[test]
    fn archived_sessions_are_never_rows() {
        let mut tree = tree();
        let mut gone = agent("gone", "w1", AgentStatus::NeedsFeedback, false, 9_000);
        gone.archived = true;
        gone.archived_at = 9_000;
        tree.agents.push(gone);
        let listed = rows(&tree, "");
        assert!(
            !listed.iter().any(|t| t.contains("gone")),
            "archived row in the overview: {listed:?}"
        );
        let hunted = rows(&tree, "gone");
        assert!(
            !hunted.iter().any(|t| t.contains("gone")),
            "archived row found by name: {hunted:?}"
        );
    }
}
