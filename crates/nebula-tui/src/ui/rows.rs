//! Shared row rendering primitives for lists, cards and modal rows.

use nebula_core::AgentStatus;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::Theme;

/// The dot. `unseen` splits the finished state in two: blue while a
/// finished turn is still unread and green once it has been read.
pub(crate) fn status_dot(status: Option<AgentStatus>, unseen: bool, th: Theme) -> Span<'static> {
    let glyph = match status {
        Some(AgentStatus::Disconnected) | None => "○ ",
        Some(_) => "● ",
    };
    Span::styled(glyph, Style::default().fg(status_color(status, unseen, th)))
}

/// The STATUS DOT's color on its own, for marks that answer to it.
pub(crate) fn status_color(status: Option<AgentStatus>, unseen: bool, th: Theme) -> Color {
    match status {
        Some(AgentStatus::Fresh) => th.dim,
        Some(AgentStatus::Running) => th.warn,
        Some(AgentStatus::Finished) if unseen => th.done,
        Some(AgentStatus::Finished) => th.ok,
        Some(AgentStatus::NeedsFeedback) => th.err,
        Some(AgentStatus::Terminated) => th.special,
        Some(AgentStatus::Disconnected) | None => th.dim,
    }
}

/// `s` cut to `max` cells with a single ellipsis marking the cut.
pub(crate) fn fit_to_width(s: &str, max: usize) -> String {
    if Span::raw(s).width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    let mut buf = [0u8; 4];
    for c in s.chars() {
        let ch: &str = c.encode_utf8(&mut buf);
        let w = Span::raw(ch).width();
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// The selection mark's color on a focused selection.
fn selection_mark(mark: Color, th: Theme) -> Color {
    if mark == th.dim {
        th.muted
    } else {
        mark
    }
}

/// Base style for a whole list row.
fn row_bar(selected: bool, focused: bool, th: Theme) -> Style {
    if selected && focused {
        Style::default().bg(th.sel_bg).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().bg(th.sel_bg_dim)
    } else {
        Style::default()
    }
}

/// Render one list row as a full-width bar. Overlay lists carry no status
/// dot, so the mark defaults to the theme accent.
pub(crate) fn render_row(
    f: &mut Frame,
    area: Rect,
    spans: Vec<Span>,
    selected: bool,
    focused: bool,
    th: Theme,
) {
    render_button(f, area, vec![spans], selected, focused, th, 0, th.accent);
}

/// Render one list entry as a button `area.height` rows tall.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_button<'a>(
    f: &mut Frame,
    area: Rect,
    mut text: Vec<Vec<Span<'a>>>,
    selected: bool,
    focused: bool,
    th: Theme,
    text_row: u16,
    mark: Color,
) {
    if selected {
        for s in text.iter_mut().flatten() {
            if s.style.fg == Some(th.dim) {
                s.style.fg = Some(th.muted);
            }
        }
    }
    let marker = || {
        if selected && focused {
            Span::styled("▌", Style::default().fg(selection_mark(mark, th)))
        } else if selected {
            Span::styled("▌", Style::default().fg(th.dim))
        } else {
            Span::raw(" ")
        }
    };
    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    for r in 0..area.height {
        let mut spans = vec![marker()];
        if let Some(row) = r
            .checked_sub(text_row)
            .and_then(|i| text.get_mut(i as usize))
        {
            spans.append(row);
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(lines).style(row_bar(selected, focused, th)),
        area,
    );
}
