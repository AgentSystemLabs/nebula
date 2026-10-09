use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::{ReviewTabKind, SessionRef};
use ratatui::layout::Rect;

use crate::app::{App, Overlay};

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewView {
    pub sessions: Vec<SessionRef>,
    pub session: usize,
    pub tabs: Vec<ReviewTabKind>,
    pub tab: usize,
    pub area: Rect,
    pub body_area: Rect,
    pub tab_hits: Vec<(u16, u16)>,
}

impl ReviewView {
    pub fn new(sessions: Vec<SessionRef>, tabs: Vec<ReviewTabKind>) -> Self {
        let sessions = if sessions.is_empty() {
            Vec::new()
        } else {
            sessions
        };
        let tabs = if tabs.is_empty() {
            vec![
                ReviewTabKind::Terminal,
                ReviewTabKind::Diff,
                ReviewTabKind::History,
                ReviewTabKind::PullRequest,
            ]
        } else {
            tabs
        };
        Self {
            sessions,
            session: 0,
            tabs,
            tab: 0,
            area: Rect::default(),
            body_area: Rect::default(),
            tab_hits: Vec::new(),
        }
    }

    pub fn selected_session(&self) -> Option<&SessionRef> {
        self.sessions.get(self.session)
    }

    pub fn selected_tab(&self) -> Option<ReviewTabKind> {
        self.tabs.get(self.tab).copied()
    }

    fn select_tab(&mut self, index: i64) {
        if self.tabs.is_empty() {
            return;
        }
        self.tab = index.rem_euclid(self.tabs.len() as i64) as usize;
    }
}

pub fn open(app: &mut App, sessions: Vec<SessionRef>, tabs: Vec<ReviewTabKind>) {
    let view = ReviewView::new(sessions, tabs);
    app.modals.overlay = Some(Overlay::Review(view));
    app.pane.term_locked = false;
    app.chrome.dirty = true;
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(Overlay::Review(view)) = &mut app.modals.overlay else {
        return false;
    };
    match key.code {
        KeyCode::Esc => {
            app.modals.overlay = None;
            app.chrome.dirty = true;
            true
        }
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            view.select_tab(view.tab as i64 - 1);
            app.chrome.dirty = true;
            true
        }
        KeyCode::Tab => {
            view.select_tab(view.tab as i64 + 1);
            app.chrome.dirty = true;
            true
        }
        KeyCode::Char(c) if ('1'..='9').contains(&c) => {
            let index = c as usize - '1' as usize;
            if index < view.tabs.len() {
                view.select_tab(index as i64);
                app.chrome.dirty = true;
            }
            true
        }
        _ => false,
    }
}

pub fn handle_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    let Some(Overlay::Review(view)) = &mut app.modals.overlay else {
        return false;
    };
    let x = mouse.column;
    let y = mouse.row;
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !contains(view.area, x, y) {
                app.modals.overlay = None;
                app.chrome.dirty = true;
                return true;
            }
            if y == view.area.y {
                if let Some((idx, _)) = view
                    .tab_hits
                    .iter()
                    .enumerate()
                    .find(|(_, (a, b))| x >= *a && x < *b)
                {
                    view.select_tab(idx as i64);
                    app.chrome.dirty = true;
                    return true;
                }
            }
            true
        }
        _ => false,
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub fn tab_label(tab: ReviewTabKind) -> &'static str {
    match tab {
        ReviewTabKind::Terminal => "Terminal",
        ReviewTabKind::Diff => "Diff",
        ReviewTabKind::History => "History",
        ReviewTabKind::PullRequest => "PR",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nebula_core::AgentId;

    fn app_with_review() -> App {
        let mut app = App::new();
        open(
            &mut app,
            vec![SessionRef::Agent(AgentId("a1".into()))],
            vec![
                ReviewTabKind::Terminal,
                ReviewTabKind::Diff,
                ReviewTabKind::History,
                ReviewTabKind::PullRequest,
            ],
        );
        app
    }

    fn tab(app: &App) -> usize {
        match &app.modals.overlay {
            Some(Overlay::Review(view)) => view.tab,
            other => panic!("expected review overlay, got {other:?}"),
        }
    }

    #[test]
    fn tab_and_number_keys_switch_tabs() {
        let mut app = app_with_review();
        assert_eq!(tab(&app), 0);
        assert!(handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
        ));
        assert_eq!(tab(&app), 1);
        assert!(handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)
        ));
        assert_eq!(tab(&app), 0);
        assert!(handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE)
        ));
        assert_eq!(tab(&app), 3);
    }

    #[test]
    fn esc_closes_review_modal() {
        let mut app = app_with_review();
        assert!(handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        ));
        assert!(app.modals.overlay.is_none());
    }
}
