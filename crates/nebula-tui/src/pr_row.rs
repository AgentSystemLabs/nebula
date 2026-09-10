//! The shape a pull request row takes in either sidebar panel — the `↗`
//! glyph, the `#42 title` label and a trailing badge — and how a draft, a
//! merged and a closed pull request are told apart from an open one. The
//! PROJECT OPEN PRS GROUP (WORKTREES PANEL) and the PR ROW (SESSIONS PANEL)
//! both build their spans here, so the two lists read as one.

use crate::pull_request::Standing;
use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// The colors of one pull request row: the arrow, the title, the PILL
/// ROW's rail and the state word in the trailing badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub glyph: Color,
    pub label: Color,
    pub rail: Color,
    pub badge: Color,
}

/// An open pull request carries the accent — the arrow says "leaves
/// nebula", the rail says it wants a reviewer. A draft is dimmed the whole
/// way down, arrow, title and rail alike: the role the PR PREVIEW paints its
/// `draft` state in, so a row that isn't ready reads as such before its
/// `draft` badge is even read. Selecting a draft row lifts it like any
/// other (`render_pill` brightens `dim` to `muted`), so it stays legible.
///
/// A merged pull request wears the PR PREVIEW's `merged` color on its arrow,
/// rail and badge, with the title left readable: the work landed, and the
/// checkout under it is the one about to be archived. A closed one keeps
/// only the arrow and badge in the preview's `closed` color and dims the
/// rest, so it reads as done-with rather than as a session needing someone
/// — the rail is the surface the STATUS DOT colors own on the rows above.
pub fn look(standing: Standing, th: Theme) -> Look {
    match standing {
        Standing::Open => Look {
            glyph: th.accent,
            label: th.muted,
            rail: th.accent,
            badge: th.dim,
        },
        Standing::Draft => Look {
            glyph: th.dim,
            label: th.dim,
            rail: th.dim,
            badge: th.dim,
        },
        Standing::Merged => Look {
            glyph: th.special,
            label: th.muted,
            rail: th.special,
            badge: th.special,
        },
        Standing::Closed => Look {
            glyph: th.err,
            label: th.dim,
            rail: th.dim,
            badge: th.err,
        },
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
            let open = look(Standing::Open, th);
            let draft = look(Standing::Draft, th);
            assert_eq!(open.glyph, th.accent, "{name}");
            assert_eq!(open.rail, th.accent, "{name}");
            assert_eq!(
                draft,
                Look {
                    glyph: th.dim,
                    label: th.dim,
                    rail: th.dim,
                    badge: th.dim,
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

    /// A merged and a closed pull request each take the color the PR
    /// PREVIEW paints that state in, on the arrow and the badge, so the row
    /// and the pane agree — and a closed one never gets the status-colored
    /// rail that would make a dead pull request look like a session that
    /// needs someone.
    #[test]
    fn merged_and_closed_rows_wear_the_preview_state_colors() {
        for name in crate::theme::THEMES {
            let th = Theme::by_name(name);
            let merged = look(Standing::Merged, th);
            assert_eq!(merged.glyph, th.special, "{name}");
            assert_eq!(merged.badge, th.special, "{name}");
            assert_eq!(merged.rail, th.special, "{name}");
            assert_eq!(merged.label, th.muted, "{name}: the title stays readable");
            let closed = look(Standing::Closed, th);
            assert_eq!(closed.glyph, th.err, "{name}");
            assert_eq!(closed.badge, th.err, "{name}");
            assert_eq!(closed.rail, th.dim, "{name}: no red rail on a closed PR");
            assert_eq!(closed.label, th.dim, "{name}");
            assert_ne!(
                merged.glyph, closed.glyph,
                "{name}: the arrow tells them apart"
            );
        }
    }

    /// The badge keeps its cell budget: a long title shortens, the `draft`
    /// mark does not fall off the end.
    #[test]
    fn the_badge_is_billed_before_the_title() {
        let th = Theme::default();
        let rows = spans(
            look(Standing::Draft, th),
            "#9 A title far too long for the column",
            20,
            Some((" draft".into(), th.dim)),
        );
        let text: String = rows.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(" draft"), "{text:?}");
        assert!(text.chars().count() <= 20, "{text:?}");
        assert_eq!(rows[0].style.fg, Some(th.dim), "a draft's arrow is dim");

        let plain = spans(look(Standing::Open, th), "#7 Attach links", 20, None);
        assert_eq!(plain.len(), 2, "no badge, no span for one");
        assert_eq!(plain[0].style.fg, Some(th.accent));
    }
}
