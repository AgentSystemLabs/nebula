use super::*;

pub(super) fn set_visible(app: &mut App, show: bool) {
    app.workflows.show = show;
    if !show && app.focus == Focus::Workflows {
        app.focus = Focus::Sessions;
    }
}

pub(super) fn select(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) -> bool {
    let Some(run) = app.workflow_rows().get(index).copied().cloned() else {
        return false;
    };
    app.workflows.selected = Some(run.id);
    app.focus = Focus::Workflows;
    let Some(worktree) = run
        .worktree
        .filter(|id| app.tree.worktrees.iter().any(|w| &w.id == id))
    else {
        app.flash = Some("WORKTREE is not available for this workflow".into());
        return false;
    };
    let stage = run.stages.get(run.current).or_else(|| run.stages.last());
    let agent = stage.and_then(|s| s.agent.as_ref()).filter(|id| {
        app.tree
            .agents
            .iter()
            .any(|a| &a.id == *id && a.worktree_id == worktree && !a.archived)
    });
    let target = agent.map_or_else(
        || PaletteTarget::Worktree(worktree.clone()),
        |id| PaletteTarget::Session(id.clone()),
    );
    jump_to_target_inner(app, target, Landing::FocusOnly, out);
    app.focus = Focus::Workflows;
    true
}
