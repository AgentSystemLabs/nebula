//! Which row of a list the pointer is on.
//!
//! Every list in the app — the modals' result lists, the pickers, the
//! panels — answers a click with the same arithmetic: the list's box, the
//! index of the first row showing in it, how many rows there are. It was
//! written out in each mouse handler, and a copy that forgot the bounds
//! check or the window offset was a click that landed on the wrong row.
//! This is the one copy.
//!
//! It answers *which row*, and nothing else. What a click on that row
//! *does* is never the mouse handler's to decide: it selects the row and
//! calls the same function the row's key does (`event_loop::activate`, a
//! modal's own `activate_selected`, a `Cmd` executor) — see the note at the
//! top of `event_loop::activate`.

use ratatui::layout::{Position, Rect};

/// The index of the row under `pos`: `first` is the index of the row drawn
/// on the list's top line (its scroll offset, or its `window_start`), `len`
/// the number of rows in the whole list. None off the list, and on the
/// blank lines under a list shorter than its box.
pub(crate) fn row_at(list: Rect, first: usize, len: usize, pos: Position) -> Option<usize> {
    if !list.contains(pos) {
        return None;
    }
    let index = first + usize::from(pos.y - list.y);
    (index < len).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_names_the_row_under_it_or_nothing() {
        let list = Rect::new(10, 5, 20, 4);
        let at = |x, y| Position::new(x, y);
        assert_eq!(row_at(list, 0, 10, at(12, 5)), Some(0));
        assert_eq!(row_at(list, 0, 10, at(12, 8)), Some(3));
        // Scrolled: the top line shows row 6.
        assert_eq!(row_at(list, 6, 10, at(12, 6)), Some(7));
        // The blank lines under a short list are not rows.
        assert_eq!(row_at(list, 0, 2, at(12, 7)), None);
        assert_eq!(row_at(list, 6, 8, at(12, 7)), None);
        // Off the list on every side.
        assert_eq!(row_at(list, 0, 10, at(12, 4)), None);
        assert_eq!(row_at(list, 0, 10, at(12, 9)), None);
        assert_eq!(row_at(list, 0, 10, at(9, 6)), None);
        assert_eq!(row_at(list, 0, 10, at(30, 6)), None);
        // A box that was never drawn has no rows to click.
        assert_eq!(row_at(Rect::default(), 0, 10, at(0, 0)), None);
    }
}
