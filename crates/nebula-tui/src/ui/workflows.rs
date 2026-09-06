use super::*;
use crate::workflows::{current_step, next_step, progress, status_label, ICON};
use nebula_core::workflow::WorkflowStatus;

const ROW_H: usize = 8;

fn color(status: WorkflowStatus, th: Theme) -> Color {
    match status {
        WorkflowStatus::Blocked => th.err,
        WorkflowStatus::Waiting | WorkflowStatus::Paused => th.warn,
        WorkflowStatus::Completed => th.ok,
        WorkflowStatus::Creating | WorkflowStatus::Running => th.accent,
    }
}

pub(super) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Workflows;
    let rows: Vec<_> = app.workflow_rows().into_iter().cloned().collect();
    let inner = draw_column(
        f,
        area,
        "WORKFLOWS",
        Some(rows.len()).filter(|n| *n > 0),
        focused,
        th,
    );
    let selected = app.workflow_selection();
    let anchor = rows.get(selected).map(|r| (r.id.clone(), selected));
    if let Some((id, _)) = &anchor {
        app.workflows.selected = Some(id.clone());
    }
    if app.workflows.anchor != anchor {
        app.workflows.anchor = anchor;
        let top = selected * ROW_H;
        if top < app.workflows.scroll {
            app.workflows.scroll = top;
        } else if top + ROW_H > app.workflows.scroll + inner.height as usize {
            app.workflows.scroll = (top + ROW_H).saturating_sub(inner.height as usize);
        }
    }
    app.workflows.scroll = app
        .workflows
        .scroll
        .min((rows.len() * ROW_H).saturating_sub(inner.height as usize));
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from("No workflows yet"),
                Line::from(""),
                Line::from("Start from a main SESSION:"),
                Line::from("/nebula-workflow <task>"),
            ])
            .style(Style::default().fg(th.muted)),
            inner,
        );
    }
    for (i, run) in rows.iter().enumerate() {
        let y = (i * ROW_H) as isize - app.workflows.scroll as isize;
        let rail = color(run.status, th);
        let project = app
            .tree
            .projects
            .iter()
            .find(|p| p.id == run.project)
            .map_or("", |p| p.name.as_str());
        let definition = run
            .name
            .as_deref()
            .or(run.workflow.as_deref())
            .unwrap_or("workflow");
        let lines = [
            format!("{ICON} {}", run.title),
            format!("{project} · {definition}"),
            status_label(run.status).to_string(),
            current_step(run),
            next_step(run),
            progress(run),
            if matches!(
                run.status,
                WorkflowStatus::Blocked | WorkflowStatus::Paused | WorkflowStatus::Waiting
            ) {
                run.message.split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                String::new()
            },
        ];
        for (offset, text) in lines.into_iter().enumerate() {
            let Some(rect) = row_rect_at(inner, y + offset as isize) else {
                continue;
            };
            let style = Style::default().fg(if offset == 0 {
                th.text
            } else if offset == 2 || offset == 6 {
                rail
            } else {
                th.muted
            });
            let text = truncate(&text, inner.width.saturating_sub(1) as usize);
            if offset == 0 {
                render_row(
                    f,
                    rect,
                    vec![Span::styled(text, style.add_modifier(Modifier::BOLD))],
                    i == selected,
                    focused,
                    th,
                );
            } else {
                f.render_widget(
                    Paragraph::new(Span::styled(format!(" {text}"), style)),
                    rect,
                );
            }
        }
        if let Some(hit) = rows_rect_at(inner, y, ROW_H as u16) {
            app.hits.push((hit, HitTarget::Workflow(i)));
        }
    }
    app.hits.push((inner, HitTarget::PanelBg(Focus::Workflows)));
}

/// Reserve a clickable progress count even before the metrics poll replies.
pub(super) fn footer(f: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let rows = app.workflow_rows();
    if rows.is_empty() && !app.workflows.show {
        return area;
    }
    let active = rows.iter().filter(|r| r.status.active()).count();
    let attention = rows
        .iter()
        .any(|r| matches!(r.status, WorkflowStatus::Blocked | WorkflowStatus::Waiting));
    let key = key_hint(app, Action::ToggleWorkflows);
    let text = format!(" {ICON} {active} running · {key} ");
    let width = (Span::raw(&text).width() as u16).min(area.width);
    let badge = Rect::new(
        area.right().saturating_sub(width),
        area.bottom().saturating_sub(1),
        width,
        area.height.min(1),
    );
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(if attention {
            app.theme.warn
        } else {
            app.theme.accent
        })),
        badge,
    );
    app.hits.push((badge, HitTarget::FooterWorkflows));
    Rect {
        width: area.width.saturating_sub(width),
        ..area
    }
}
