//! Per-repository PR choices before creating a session workspace.

use crossterm::event::{KeyCode, KeyEvent};
use nebula_core::{ClientRequest, WorkspaceCheckout, WorkspaceCheckoutSelection};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::{
    app::{App, Overlay, WorkspacePrAnswer, WorkspacePrLoadState, WorkspacePrPicker},
    theme::Theme,
};

pub(crate) fn open(
    app: &mut App,
    project: nebula_core::ProjectId,
    name: String,
    focus: crate::app::Focus,
    out: &mut Vec<ClientRequest>,
) {
    start(app, project, name, focus, None, out);
}

/// Local heads that can be checked out into another worktree. Git refuses a
/// branch already attached to a worktree, so leave those out of this list.
async fn local_branches(dir: &std::path::Path) -> Option<Vec<String>> {
    let refs = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .output()
        .await
        .ok()?;
    if !refs.status.success() {
        return None;
    }
    let worktrees = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
        .ok()?;
    if !worktrees.status.success() {
        return None;
    }
    let occupied: std::collections::HashSet<String> = String::from_utf8_lossy(&worktrees.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("branch refs/heads/"))
        .map(str::to_string)
        .collect();
    Some(
        String::from_utf8_lossy(&refs.stdout)
            .lines()
            .map(str::trim)
            .filter(|branch| !branch.is_empty() && !occupied.contains(*branch))
            .map(str::to_string)
            .collect(),
    )
}

pub(crate) fn open_for_launch(
    app: &mut App,
    project: nebula_core::ProjectId,
    name: String,
    launch: crate::quick_prompt::QuickLaunch,
    text: String,
    focus: bool,
    follow: bool,
    out: &mut Vec<ClientRequest>,
) {
    start(
        app,
        project,
        name,
        crate::app::Focus::Sessions,
        Some((launch, text, focus, follow)),
        out,
    );
}

fn start(
    app: &mut App,
    project: nebula_core::ProjectId,
    name: String,
    focus: crate::app::Focus,
    launch: Option<(crate::quick_prompt::QuickLaunch, String, bool, bool)>,
    out: &mut Vec<ClientRequest>,
) {
    let project_row = app
        .tree
        .projects
        .iter()
        .find(|row| row.id == project)
        .cloned();
    let repositories: Vec<_> = app
        .tree
        .repositories
        .iter()
        .filter(|repository| repository.project_id == project)
        .cloned()
        .collect();
    if app.workspace_pr_tx.is_none() {
        submit_without_picker(app, project, name, focus, launch, out);
        return;
    }
    if repositories.is_empty() {
        submit_without_picker(app, project, name, focus, launch, out);
        return;
    }
    let picker = WorkspacePrPicker {
        project: project.clone(),
        project_name: project_row.map_or_else(|| project.to_string(), |row| row.name),
        name,
        focus,
        loads: vec![WorkspacePrLoadState::Loading; repositories.len()],
        selected: vec![None; repositories.len()],
        repository_index: 0,
        option_index: 0,
        repositories,
        launch,
        area: Rect::default(),
    };
    let tx = app.workspace_pr_tx.clone();
    if let Some(tx) = tx {
        for repository in &picker.repositories {
            let tx = tx.clone();
            let dir = repository.path.clone();
            let project = project.clone();
            let name = repository.name.clone();
            tokio::spawn(async move {
                let (prs, branches) =
                    tokio::join!(crate::pull_request::list(&dir), local_branches(&dir),);
                let _ = tx.send(WorkspacePrAnswer {
                    project,
                    repository: name,
                    prs,
                    branches,
                });
            });
        }
    }
    app.overlay = Some(Overlay::WorkspacePrPicker(picker));
    app.dirty = true;
}

fn submit_without_picker(
    app: &mut App,
    project: nebula_core::ProjectId,
    name: String,
    focus: crate::app::Focus,
    launch: Option<(crate::quick_prompt::QuickLaunch, String, bool, bool)>,
    out: &mut Vec<ClientRequest>,
) {
    let placeholder =
        crate::event_loop::stage_session_workspace_placeholder(app, project.clone(), &name);
    crate::event_loop::select_worktree_by_id(app, &placeholder, out);
    let (intent, follow) = match launch {
        Some((launch, text, focus, follow)) => (
            crate::app::PendingIntent::CreateSessionWorkspaceAndLaunch {
                launch,
                text,
                focus,
                follow,
                placeholder,
            },
            follow,
        ),
        None => (
            crate::app::PendingIntent::CreateSessionWorkspace {
                project: project.clone(),
                name: name.clone(),
                focus,
                placeholder,
            },
            true,
        ),
    };
    let req_id = app.alloc_req_id(intent);
    if !follow {
        app.left_behind.insert(req_id);
    }
    app.flash = Some(format!("creating session workspace {name}…"));
    out.push(ClientRequest::CreateSessionWorkspace {
        req_id,
        project,
        name,
    });
}

pub(crate) fn land(app: &mut App, answer: WorkspacePrAnswer) {
    let Some(Overlay::WorkspacePrPicker(view)) = &mut app.overlay else {
        return;
    };
    if view.project != answer.project {
        return;
    }
    let Some(index) = view
        .repositories
        .iter()
        .position(|repo| repo.name == answer.repository)
    else {
        return;
    };
    view.loads[index] = match (answer.prs, answer.branches) {
        (Some(prs), Some(mut branches)) => {
            branches.retain(|branch| !prs.iter().any(|pr| pr.head == *branch));
            WorkspacePrLoadState::Loaded { prs, branches }
        }
        (Some(prs), None) => WorkspacePrLoadState::Loaded {
            prs,
            branches: vec![],
        },
        (None, Some(branches)) => WorkspacePrLoadState::Loaded {
            prs: vec![],
            branches,
        },
        (None, None) => WorkspacePrLoadState::Failed,
    };
    view.option_index = view
        .option_index
        .min(option_count(view, index).saturating_sub(1));
    app.dirty = true;
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::WorkspacePrPicker(view)) = &mut app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Esc => {
            let back = match &app.overlay {
                Some(Overlay::WorkspacePrPicker(view)) => view.launch.clone(),
                _ => None,
            };
            app.overlay = None;
            if let Some((launch, text, _, _)) = back {
                crate::quick_prompt::reopen(app, launch, &text);
            }
        }
        KeyCode::Tab | KeyCode::Right => {
            if view.repositories.is_empty() {
                return;
            }
            view.repository_index = (view.repository_index + 1) % view.repositories.len();
            view.option_index = view.selected[view.repository_index]
                .as_ref()
                .and_then(|selected| selected_index(view, view.repository_index, selected))
                .unwrap_or(0);
        }
        KeyCode::BackTab | KeyCode::Left => {
            if view.repositories.is_empty() {
                return;
            }
            view.repository_index = view
                .repository_index
                .checked_sub(1)
                .unwrap_or(view.repositories.len() - 1);
            view.option_index = view.selected[view.repository_index]
                .as_ref()
                .and_then(|selected| selected_index(view, view.repository_index, selected))
                .unwrap_or(0);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            view.option_index = (view.option_index + 1)
                .min(option_count(view, view.repository_index).saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => {
            view.option_index = view.option_index.saturating_sub(1);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if view.repositories.is_empty() {
                submit(app, out);
                return;
            }
            if view.repository_index + 1 == view.repositories.len()
                && view
                    .loads
                    .iter()
                    .any(|state| matches!(state, WorkspacePrLoadState::Loading))
            {
                app.flash = Some("still loading pull requests…".into());
                return;
            }
            let chosen = match view.loads[view.repository_index].clone() {
                WorkspacePrLoadState::Loaded { prs, branches } if view.option_index > 0 => {
                    let branch_index = view.option_index - 1;
                    let repository = view.repositories[view.repository_index].name.clone();
                    if let Some(pr) = prs.get(branch_index) {
                        Some(WorkspaceCheckoutSelection {
                            repository,
                            checkout: WorkspaceCheckout::PullRequest {
                                number: pr.number,
                                url: pr.url.clone(),
                                head: pr.head.clone(),
                            },
                        })
                    } else {
                        branches
                            .get(branch_index.saturating_sub(prs.len()))
                            .map(|branch| WorkspaceCheckoutSelection {
                                repository,
                                checkout: WorkspaceCheckout::LocalBranch {
                                    branch: branch.clone(),
                                },
                            })
                    }
                }
                _ => None,
            };
            view.selected[view.repository_index] = chosen;
            if view.repository_index + 1 < view.repositories.len() {
                view.repository_index += 1;
                view.option_index = view.selected[view.repository_index]
                    .as_ref()
                    .and_then(|selected| selected_index(view, view.repository_index, selected))
                    .unwrap_or(0);
            } else {
                submit(app, out);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

fn selected_index(
    view: &WorkspacePrPicker,
    index: usize,
    selected: &WorkspaceCheckoutSelection,
) -> Option<usize> {
    let WorkspacePrLoadState::Loaded { prs, branches } = &view.loads[index] else {
        return None;
    };
    match &selected.checkout {
        WorkspaceCheckout::PullRequest { number, .. } => prs
            .iter()
            .position(|pr| pr.number == *number)
            .map(|index| index + 1),
        WorkspaceCheckout::LocalBranch { branch } => branches
            .iter()
            .position(|candidate| candidate == branch)
            .map(|index| prs.len() + index + 1),
    }
}

fn option_count(view: &WorkspacePrPicker, index: usize) -> usize {
    match &view.loads[index] {
        WorkspacePrLoadState::Loaded { prs, branches } => prs.len() + branches.len() + 1,
        _ => 1,
    }
}

fn submit(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::WorkspacePrPicker(view)) = app.overlay.take() else {
        return;
    };
    let prs: Vec<_> = view.selected.into_iter().flatten().collect();
    let placeholder = crate::event_loop::stage_session_workspace_placeholder(
        app,
        view.project.clone(),
        &view.name,
    );
    let (intent, follow) = match view.launch {
        Some((launch, text, focus, follow)) => (
            crate::app::PendingIntent::CreateSessionWorkspaceAndLaunch {
                launch,
                text,
                focus,
                follow,
                placeholder,
            },
            follow,
        ),
        None => (
            crate::app::PendingIntent::CreateSessionWorkspace {
                project: view.project.clone(),
                name: view.name.clone(),
                focus: view.focus,
                placeholder,
            },
            true,
        ),
    };
    let req_id = app.alloc_req_id(intent);
    if !follow {
        app.left_behind.insert(req_id);
    }
    app.flash = Some(format!("creating session workspace {}…", view.name));
    out.push(if prs.is_empty() {
        ClientRequest::CreateSessionWorkspace {
            req_id,
            project: view.project,
            name: view.name,
        }
    } else {
        ClientRequest::CreateSessionWorkspaceWithSelections {
            req_id,
            project: view.project,
            name: view.name,
            selections: prs,
        }
    });
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &WorkspacePrPicker, theme: Theme) {
    let area = centered_rect(f.area(), 72, 70);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(format!(
            " Workspace checkouts · {} · {} ",
            view.project_name, view.name
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    if view.repositories.is_empty() {
        f.render_widget(
            Paragraph::new("No repository PRs to choose · creating with the configured base"),
            chunks[0],
        );
        f.render_widget(
            Paragraph::new("Enter: create  Esc: cancel").style(Style::default().fg(theme.dim)),
            chunks[1],
        );
        if let Some(Overlay::WorkspacePrPicker(live)) = &mut app.overlay {
            live.area = area;
        }
        return;
    }
    let repository = &view.repositories[view.repository_index];
    let choices: Vec<(String, bool)> = match &view.loads[view.repository_index] {
        WorkspacePrLoadState::Loading => vec![("Loading pull requests…".into(), false)],
        WorkspacePrLoadState::Failed => {
            vec![("No PR (pull-request list unavailable)".into(), true)]
        }
        WorkspacePrLoadState::Loaded { prs, branches } => {
            std::iter::once(("Default base · no PR or branch".into(), true))
                .chain(prs.iter().map(|pr| {
                    (
                        format!(
                            "#{} {}{}",
                            pr.number,
                            pr.title,
                            if pr.is_draft { " · draft" } else { "" }
                        ),
                        view.selected[view.repository_index]
                            .as_ref()
                            .is_some_and(|selected| matches!(
                                selected.checkout,
                                WorkspaceCheckout::PullRequest { number, .. } if number == pr.number
                            )),
                    )
                }))
                .chain(branches.iter().map(|branch| {
                    (
                        format!("{branch} · local branch"),
                        view.selected[view.repository_index]
                            .as_ref()
                            .is_some_and(|selected| {
                                matches!(
                                    &selected.checkout,
                                    WorkspaceCheckout::LocalBranch { branch: selected_branch }
                                        if selected_branch == branch
                                )
                            }),
                    )
                }))
                .collect()
        }
    };
    let heading = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("{}  ", repository.name),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "Repository {} of {}",
                view.repository_index + 1,
                view.repositories.len()
            ),
            Style::default().fg(theme.dim),
        ),
    ]));
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(chunks[0]);
    f.render_widget(heading, body[0]);
    let visible = body[1].height as usize;
    let first = view.option_index.saturating_sub(visible.saturating_sub(1));
    let rows: Vec<Line> = choices
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, (label, chosen))| {
            let active = index == view.option_index;
            let icon = if active {
                "›"
            } else if *chosen {
                "●"
            } else {
                "○"
            };
            let style = if active {
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else if *chosen {
                Style::default().fg(theme.ok)
            } else {
                Style::default().fg(theme.text)
            };
            Line::from(Span::styled(format!(" {icon} {label}"), style))
        })
        .collect();
    f.render_widget(Paragraph::new(rows), body[1]);
    let hint = "↑/↓ choose PR or local branch  Tab/←/→ repository  Enter: next/create  Esc: cancel";
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(theme.dim)),
        chunks[1],
    );
    if let Some(Overlay::WorkspacePrPicker(live)) = &mut app.overlay {
        live.area = area;
    }
}

fn centered_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_pct) / 2),
            Constraint::Percentage(height_pct),
            Constraint::Percentage((100 - height_pct) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_pct) / 2),
            Constraint::Percentage(width_pct),
            Constraint::Percentage((100 - width_pct) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Focus, PendingIntent};
    use crossterm::event::{KeyEvent, KeyModifiers};
    use nebula_core::{Project, ProjectId, ProjectRepository};

    #[test]
    fn confirms_one_pr_per_repository_and_keeps_unselected_members_on_default_base() {
        let mut app = App::new();
        let project = ProjectId("project".into());
        app.tree.projects.push(Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        app.tree.repositories = ["api", "web"]
            .into_iter()
            .map(|name| ProjectRepository {
                project_id: project.clone(),
                name: name.into(),
                path: format!("/tmp/demo/{name}").into(),
                branch: "main".into(),
                changed_files: None,
            })
            .collect();
        let pr = crate::pull_request::OpenPr {
            number: 42,
            title: "Fix login".into(),
            url: "https://github.com/example/api/pull/42".into(),
            is_draft: false,
            health: Default::default(),
            head: "fix-login".into(),
        };
        app.overlay = Some(Overlay::WorkspacePrPicker(WorkspacePrPicker {
            project: project.clone(),
            project_name: "demo".into(),
            name: "fix-auth".into(),
            focus: Focus::Worktrees,
            repositories: app.tree.repositories.clone(),
            loads: vec![
                WorkspacePrLoadState::Loaded {
                    prs: vec![pr],
                    branches: vec!["fix-login".into()],
                },
                WorkspacePrLoadState::Loaded {
                    prs: vec![],
                    branches: vec![],
                },
            ],
            selected: vec![None, None],
            repository_index: 0,
            option_index: 1,
            launch: None,
            area: Rect::default(),
        }));
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );

        assert!(matches!(
            out.as_slice(),
            [ClientRequest::CreateSessionWorkspaceWithSelections { project: got_project, name, selections, .. }]
                if got_project == &project
                    && name == "fix-auth"
                    && selections == &[WorkspaceCheckoutSelection {
                        repository: "api".into(),
                        checkout: WorkspaceCheckout::PullRequest {
                            number: 42,
                            url: "https://github.com/example/api/pull/42".into(),
                            head: "fix-login".into(),
                        },
                    }]
        ));
        assert!(matches!(
            app.pending.values().next(),
            Some(PendingIntent::CreateSessionWorkspace { .. })
        ));
    }

    #[test]
    fn confirms_a_local_branch_checkout() {
        let project = ProjectId("project".into());
        let repository = ProjectRepository {
            project_id: project.clone(),
            name: "api".into(),
            path: "/tmp/demo/api".into(),
            branch: "main".into(),
            changed_files: None,
        };
        let mut app = App::new();
        app.overlay = Some(Overlay::WorkspacePrPicker(WorkspacePrPicker {
            project: project.clone(),
            project_name: "demo".into(),
            name: "fix-auth".into(),
            focus: Focus::Worktrees,
            repositories: vec![repository],
            loads: vec![WorkspacePrLoadState::Loaded {
                prs: vec![],
                branches: vec!["feature/login".into()],
            }],
            selected: vec![None],
            repository_index: 0,
            option_index: 1,
            launch: None,
            area: Rect::default(),
        }));
        let mut out = Vec::new();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut out,
        );

        assert!(matches!(
            out.as_slice(),
            [ClientRequest::CreateSessionWorkspaceWithSelections { selections, .. }]
                if selections == &[WorkspaceCheckoutSelection {
                    repository: "api".into(),
                    checkout: WorkspaceCheckout::LocalBranch {
                        branch: "feature/login".into(),
                    },
                }]
        ));
    }
}
