//! Extracted event-loop helper section.

use super::*;

pub(crate) fn open_follow_up(app: &mut App, id: AgentId, text: String) {
    open_prompt(app, PromptKind::FollowUp { id });
    if let Some(Overlay::Prompt(prompt)) = &mut app.modals.overlay {
        prompt.input = crate::text_input::TextInput::multiline_with_text(text);
    }
}

// ---- overlays ----

/// New-worktree prompt with a random branch name already picked out.
/// The project's existing branches are excluded, so Enter on an empty
/// input can't land on a name `git worktree add` would reject.
pub(crate) fn open_new_worktree_prompt(app: &mut App, project: nebula_core::ProjectId) {
    let suggestion = crate::branch_name::random_name(&app.project_branches(&project));
    open_prompt(
        app,
        PromptKind::NewWorktree {
            project,
            suggestion,
        },
    );
}

/// The round trip a QUICK PROMPT's picker carries: the launch as it stands
/// and the text typed so far.
pub(crate) fn quick_return_of(prompt: &PromptDialog) -> Option<crate::quick_prompt::QuickReturn> {
    let PromptKind::QuickPrompt(launch) = &prompt.kind else {
        return None;
    };
    Some(crate::quick_prompt::QuickReturn {
        launch: launch.clone(),
        text: prompt.input.as_str().to_string(),
        from_box: true,
    })
}

pub(crate) fn open_prompt(app: &mut App, kind: PromptKind) {
    use std::borrow::Cow;
    let (title, label, input): (Cow<'static, str>, Cow<'static, str>, String) = match &kind {
        // Starts on the folder nebula was started in — its parent listed,
        // the folder itself highlighted, so Enter opens it and ↑/↓ reach
        // the repos beside it — or at "~/" with the home listing showing.
        // Typing a leading '/' or '~' replaces the prefill (see the Char
        // arm), and Ctrl+u clears it.
        PromptKind::AddProject => (
            "Open project".into(),
            "a folder with a git repository — Enter opens it".into(),
            add_project_prefill(app),
        ),
        PromptKind::SetProjectPath { old_path, .. } => (
            "Locate project".into(),
            format!("new folder for {}", old_path.display()).into(),
            repath_prefill(old_path),
        ),
        PromptKind::NewWorktree { suggestion, .. } => (
            "New worktree".into(),
            format!("branch name (empty = {suggestion})").into(),
            String::new(),
        ),
        PromptKind::ClaudeCloudTask { .. } => (
            "Claude Cloud task".into(),
            "what should Claude do?".into(),
            String::new(),
        ),
        PromptKind::AgentPresetTask { preset, .. } => (
            format!("Task for {}", preset.name).into(),
            if preset.has_wrapping() {
                format!(
                    "{} — prefix + your task + postfix (empty = prefix + postfix only)",
                    preset.spec_label()
                )
                .into()
            } else {
                format!(
                    "{} — sent as the first prompt (empty = start with no prompt)",
                    preset.spec_label()
                )
                .into()
            },
            String::new(),
        ),
        PromptKind::QuickPrompt(launch) => {
            (launch.title().into(), launch.label().into(), String::new())
        }
        PromptKind::CloudMessage { .. } => (
            "Send to cloud session".into(),
            "message for the cloud agent".into(),
            String::new(),
        ),
        PromptKind::FollowUp { id } => (
            format!(
                "Follow-up · {}",
                app.tree
                    .agents
                    .iter()
                    .find(|a| &a.id == id)
                    .map_or("session", |a| a.name.as_str())
            )
            .into(),
            "the agent's next turn — Enter sends it straight to its CLI".into(),
            String::new(),
        ),
        PromptKind::PrComment { label, .. } => (
            format!("Comment on {label}").into(),
            "posted on the pull request as you, through gh — markdown".into(),
            String::new(),
        ),
        PromptKind::RenameAgent { id } => {
            let current = app
                .tree
                .agents
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.name.clone())
                .unwrap_or_default();
            ("Rename agent".into(), "name".into(), current)
        }
        PromptKind::RenameTerminal { id } => {
            let current = app
                .tree
                .terminals
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            ("Rename terminal".into(), "name".into(), current)
        }
        PromptKind::RenameProject { id } => {
            let current = app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            (
                "Rename project".into(),
                "name (empty resets to the folder name)".into(),
                current,
            )
        }
        PromptKind::SettingText { kind, project } => {
            // Pre-filled with the stored value, not its display label: an
            // empty row reads `auto` on the overlay but edits as "". A
            // PROJECT TAB row's is the project's own, and the title names
            // the project so a command typed here reads as that project's.
            let cfg = crate::config::Config::load();
            let label = crate::config::spec_for(*kind)
                .map(|s| s.label)
                .unwrap_or("Setting");
            let title = match project.as_deref().and_then(|path| {
                app.tree
                    .projects
                    .iter()
                    .find(|p| p.repo_path == path)
                    .map(|p| p.name.clone())
            }) {
                Some(name) => format!("{label} · {name}"),
                None => label.to_string(),
            };
            let hint = match kind {
                crate::config::SettingKind::WorktreeBaseBranch => {
                    "branch new worktrees start from (empty = auto: origin's default branch)"
                }
                crate::config::SettingKind::RunCommand => {
                    "shell line r runs in this project's worktrees (empty = the checkout's .nebula.json \"run\")"
                }
                crate::config::SettingKind::OpenCommand => {
                    "shell line Shift+Enter / Shift+O runs to open a worktree of this project (empty = the checkout's .nebula.json \"open\")"
                }
                _ => "value (empty = default)",
            };
            let value = match project {
                Some(path) => cfg.project_text_value(path, *kind),
                None => cfg.text_value(*kind),
            };
            (title.into(), hint.into(), value)
        }

        PromptKind::IssueComment { issue, .. } => {
            let title = if issue.title.trim().is_empty() {
                format!("Comment on issue #{}", issue.number)
            } else {
                format!(
                    "Comment on issue #{} · {}",
                    issue.number,
                    crate::ui::truncate(issue.title.trim(), 40)
                )
            };
            (
                title.into(),
                format!(
                    "what do you want to say on #{}? (posted as you, with gh issue comment)",
                    issue.number
                )
                .into(),
                String::new(),
            )
        }
        PromptKind::EditLink { id } => {
            let current = app
                .tree
                .links
                .iter()
                .find(|l| &l.id == id)
                .map(|l| l.url.clone())
                .unwrap_or_default();
            ("Edit link".into(), "URL".into(), current)
        }
    };
    let highlight = matches!(kind, PromptKind::AddProject)
        .then(|| app.launch_repo_name())
        .flatten();
    let mut dialog = PromptDialog::new(title, label, input, kind);
    // The folder nebula was started in, lit in its parent's listing.
    if let Some(name) = highlight {
        dialog.hover = dialog.dirs.iter().position(|d| d.name == name);
    }
    app.modals.overlay = Some(Overlay::Prompt(dialog));
}

pub(crate) fn repath_prefill(old_path: &std::path::Path) -> String {
    let home = nebula_core::env::home_dir();
    let parent = old_path.parent().filter(|p| p.exists());
    match (parent, home.as_deref()) {
        (Some(parent), Some(home)) => match parent.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~/".to_string(),
            Ok(rest) => format!("~/{}/", rest.display()),
            Err(_) => format!("{}/", parent.display()),
        },
        (Some(parent), None) => format!("{}/", parent.display()),
        (None, Some(_)) => "~/".to_string(),
        (None, None) => String::new(),
    }
}

pub(crate) fn prompt_for_missing_project_path(app: &mut App, id: &ProjectId) -> bool {
    #[cfg(test)]
    if !app.launcher.prompt_missing_project_paths {
        return false;
    }
    let Some(project) = app.tree.projects.iter().find(|p| &p.id == id) else {
        return false;
    };
    if project.repo_path.exists() || app.launcher.dismissed_repath_projects.contains(id) {
        return false;
    }
    app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Project folder not found".into(),
        message: format!(
            "The original directory for '{}' is no longer found:\n{}\n\nPoint this project to its new folder?",
            project.name,
            project.repo_path.display()
        ),
        action: PendingAction::LocateProjectPath {
            id: id.clone(),
            old_path: project.repo_path.clone(),
        },
        area: ratatui::layout::Rect::default(),
    }));
    app.chrome.dirty = true;
    true
}

/// Where the open-project prompt starts: the parent of the repo nebula was
/// started in (`~`-relative, trailing slash, so its listing is what shows)
/// while that repo is not a project yet, else the home directory.
pub(crate) fn add_project_prefill(app: &App) -> String {
    let home = nebula_core::env::home_dir();
    let parent = app
        .launch_repo_name()
        .and(app.launcher.launch_repo.as_deref())
        .and_then(std::path::Path::parent);
    match (parent, home.as_deref()) {
        (Some(parent), Some(home)) => match parent.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~/".to_string(),
            Ok(rest) => format!("~/{}/", rest.display()),
            Err(_) => format!("{}/", parent.display()),
        },
        (Some(parent), None) => format!("{}/", parent.display()),
        (None, Some(_)) => "~/".to_string(),
        (None, None) => String::new(),
    }
}

/// The keys the all-tabs-closed SPLASH answers ([`App::projects_closed`]):
/// the ones that open a project — Enter, `n`, `o`, `+`, a `/` pick, the
/// attention walk — and the ones that only put a modal up.
pub(crate) fn opens_from_closed_splash(action: crate::keymap::Action) -> bool {
    use crate::keymap::Action;
    matches!(
        action,
        Action::Activate
            | Action::New
            | Action::AddProject
            | Action::ProjectDropdown
            | Action::Palette
            | Action::NextAttention
            | Action::PrevAttention
            | Action::Quit
            | Action::Help
            | Action::Settings
            | Action::Metrics
            | Action::Hosts
            | Action::AgentPresets
    )
}
