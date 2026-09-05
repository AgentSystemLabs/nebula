//! The shape a pull request row takes in either sidebar panel — the `↗`
//! glyph, the `#42 title` label and a trailing badge — and how a draft is
//! told apart from a finished one. The PROJECT OPEN PRS GROUP (WORKTREES
//! PANEL) and the PR ROW (SESSIONS PANEL) both build their spans here, so
//! the two lists read as one.

use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// The colors of one pull request row: the arrow, the title and the PILL
/// ROW's rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub glyph: Color,
    pub label: Color,
    pub rail: Color,
}

/// A finished pull request carries the accent — the arrow says "leaves
/// nebula", the rail says it wants a reviewer. A draft is dimmed the whole
/// way down, arrow, title and rail alike: the role the PR PREVIEW paints its
/// `draft` state in, so a row that isn't ready reads as such before its
/// `draft` badge is even read. Selecting a draft row lifts it like any
/// other (`render_pill` brightens `dim` to `muted`), so it stays legible.
pub fn look(is_draft: bool, th: Theme) -> Look {
    if is_draft {
        Look {
            glyph: th.dim,
            label: th.dim,
            rail: th.dim,
        }
    } else {
        Look {
            glyph: th.accent,
            label: th.muted,
            rail: th.accent,
        }
    }
}

/// The row's spans: `↗ `, the label cut to what `width` leaves after the
/// badge, then the badge (text and color) when there is one. The badge is
/// billed before the label so it never clips off the end of a narrow column.
pub fn spans(
    look: Look,
    label: &str,
    width: usize,
    badge: Option<(String, Color)>,
) -> Vec<Span<'static>> {
    let badge_len = badge.as_ref().map_or(0, |(b, _)| b.chars().count());
    let label_max = width.saturating_sub(3).saturating_sub(badge_len);
    let mut spans = vec![
        Span::styled("↗ ", Style::default().fg(look.glyph)),
        Span::styled(
            crate::ui::truncate(label, label_max),
            Style::default().fg(look.label),
        ),
    ];
    if let Some((badge, color)) = badge {
        spans.push(Span::styled(badge, Style::default().fg(color)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A draft is a different color from a finished pull request in every
    /// part of the row — arrow, title, rail — in every theme preset, and
    /// that color is the dim one, not a status color that would make a
    /// draft look like it needs someone.
    #[test]
    fn a_draft_is_dimmed_where_an_open_pull_request_is_accented() {
        for name in crate::theme::THEMES {
            let th = Theme::by_name(name);
            let open = look(false, th);
            let draft = look(true, th);
            assert_eq!(open.glyph, th.accent, "{name}");
            assert_eq!(open.rail, th.accent, "{name}");
            assert_eq!(
                draft,
                Look {
                    glyph: th.dim,
                    label: th.dim,
                    rail: th.dim
                },
                "{name}"
            );
            assert_ne!(
                open.glyph, draft.glyph,
                "{name}: the arrow tells them apart"
            );
            assert_ne!(open.label, draft.label, "{name}: so does the title");
        }
    }

    /// The badge keeps its cell budget: a long title shortens, the `draft`
    /// mark does not fall off the end.
    #[test]
    fn the_badge_is_billed_before_the_title() {
        let th = Theme::default();
        let rows = spans(
            look(true, th),
            "#9 A title far too long for the column",
            20,
            Some((" draft".into(), th.dim)),
        );
        let text: String = rows.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(" draft"), "{text:?}");
        assert!(text.chars().count() <= 20, "{text:?}");
        assert_eq!(rows[0].style.fg, Some(th.dim), "a draft's arrow is dim");

        let plain = spans(look(false, th), "#7 Attach links", 20, None);
        assert_eq!(plain.len(), 2, "no badge, no span for one");
        assert_eq!(plain[0].style.fg, Some(th.accent));
    }
}
