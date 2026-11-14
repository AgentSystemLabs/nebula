//! Shared fuzzy-list state for modal overlays.
//!
//! A modal still owns what choosing a row means, plus its draw/key/mouse
//! entry points. This type owns the repeatable mechanics: the query, the
//! filtered rows, the cursor, the visible window and row hit-testing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};

use crate::text_input::TextInput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterMatch {
    /// Index into [`FilterList::items`].
    pub item: usize,
    /// Character positions in the item's label matched by the query.
    pub positions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterList<T> {
    pub items: Vec<T>,
    pub query: TextInput,
    pub matches: Vec<FilterMatch>,
    /// Cursor into `matches`.
    pub cursor: usize,
    /// First visible row as of the last draw.
    pub scroll: usize,
    /// Screen rect of the result rows, written during draw for hit-testing.
    pub list_area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterListKey {
    None,
    Close,
    Cleared,
    Moved,
    QueryChanged,
}

impl<T> Default for FilterList<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            query: TextInput::new(),
            matches: Vec::new(),
            cursor: 0,
            scroll: 0,
            list_area: Rect::default(),
        }
    }
}

impl<T> FilterList<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn query_has_words(&self) -> bool {
        self.query.split_whitespace().next().is_some()
    }

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.cursor = self.cursor.min(self.matches.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&T> {
        let m = self.matches.get(self.cursor)?;
        self.items.get(m.item)
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.cursor).map(|m| m.item)
    }

    pub fn select(&mut self, index: i64) -> bool {
        let next = clamp_selection(index, self.matches.len());
        let changed = next != self.cursor;
        self.cursor = next;
        changed
    }

    pub fn step(&mut self, delta: i64) -> bool {
        self.select(self.cursor as i64 + delta)
    }

    pub fn move_to_item(&mut self, item: usize) -> bool {
        if let Some(row) = self.matches.iter().position(|m| m.item == item) {
            let changed = self.cursor != row;
            self.cursor = row;
            changed
        } else {
            false
        }
    }

    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor, height)
    }

    pub fn sync_scroll(&mut self, height: usize) -> usize {
        self.scroll = self.window_start(height);
        self.scroll
    }

    pub fn hit(&self, pos: Position) -> Option<usize> {
        crate::list_hit::row_at(self.list_area, self.scroll, self.matches.len(), pos)
    }

    pub fn hit_item(&self, pos: Position) -> Option<usize> {
        let row = self.hit(pos)?;
        self.matches.get(row).map(|m| m.item)
    }

    pub fn replace_matches(&mut self, matches: Vec<FilterMatch>) {
        self.matches = matches;
        self.cursor = clamp_selection(self.cursor as i64, self.matches.len());
        self.scroll = self.scroll.min(self.matches.len().saturating_sub(1));
    }

    pub fn apply_filter_by(
        &mut self,
        labels: impl IntoIterator<Item = String>,
        rank: impl Fn(usize) -> usize,
    ) {
        let labels: Vec<String> = labels.into_iter().collect();
        let matches = crate::fuzzy::rank_by(
            self.query.as_str(),
            labels.iter().map(String::as_str),
            |i, _| rank(i),
        )
        .into_iter()
        .map(|(item, positions)| FilterMatch { item, positions })
        .collect();
        self.replace_matches(matches);
    }

    pub fn apply_filter(&mut self, label: impl Fn(&T) -> String) {
        let labels: Vec<String> = self.items.iter().map(label).collect();
        self.apply_filter_by(labels, |i| i);
    }

    /// Apply the common modal keys: two-stage Esc, Up/Down, Ctrl+n/p and
    /// text input. PageUp/PageDown are included because the branch switcher
    /// and long file lists have always treated them as larger cursor moves.
    pub fn handle_standard_key(&mut self, key: &KeyEvent, page: i64) -> FilterListKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                FilterListKey::Cleared
            }
            KeyCode::Esc => FilterListKey::Close,
            KeyCode::Down => {
                self.step(1);
                FilterListKey::Moved
            }
            KeyCode::Up => {
                self.step(-1);
                FilterListKey::Moved
            }
            KeyCode::Char('n') if ctrl => {
                self.step(1);
                FilterListKey::Moved
            }
            KeyCode::Char('p') if ctrl => {
                self.step(-1);
                FilterListKey::Moved
            }
            KeyCode::PageDown => {
                self.step(page.max(1));
                FilterListKey::Moved
            }
            KeyCode::PageUp => {
                self.step(-page.max(1));
                FilterListKey::Moved
            }
            _ => {
                if self.query.handle_key(key).changed() {
                    FilterListKey::QueryChanged
                } else {
                    FilterListKey::None
                }
            }
        }
    }
}

pub fn clamp_selection(index: i64, len: usize) -> usize {
    let max = len.saturating_sub(1) as i64;
    index.clamp(0, max) as usize
}

pub fn window_start(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}
