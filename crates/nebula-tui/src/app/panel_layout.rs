use super::{App, MIN_PANEL_W, MIN_TERM_W};

impl App {
    /// Visible sidebar indices, left to right. Sessions is always present.
    pub fn visible_panel_indices(&self) -> Vec<usize> {
        [0, 1, 3, 2]
            .into_iter()
            .filter(|idx| self.panel_visible(*idx))
            .collect()
    }

    pub fn panel_visible(&self, idx: usize) -> bool {
        match idx {
            0 => !self.hide_projects,
            1 => !self.hide_worktrees,
            2 => true,
            3 => self.workflows.show,
            _ => false,
        }
    }

    /// Every visible sidebar owns the draggable boundary on its right.
    pub fn splitter_indices(&self) -> Vec<usize> {
        self.visible_panel_indices()
    }

    /// Screen x of splitter `idx` — the column where the panel to its right
    /// starts, i.e. the right edge of panel `idx`.
    pub fn splitter_x(&self, idx: usize) -> u16 {
        self.panel_width(idx)
            + self
                .visible_panel_indices()
                .into_iter()
                .take_while(|visible| *visible != idx)
                .map(|visible| self.panel_width(visible))
                .sum::<u16>()
    }

    /// Move splitter `idx` so its boundary lands at `boundary_x`, clamped so
    /// the panel keeps `MIN_PANEL_W` and the terminal pane keeps `MIN_TERM_W`.
    pub fn set_splitter(&mut self, idx: usize, boundary_x: i32, body_w: u16) {
        let want = boundary_x.max(0) as u16;
        if !self.panel_visible(idx) {
            return;
        }
        let visible = self.visible_panel_indices();
        let position = visible.iter().position(|i| *i == idx).unwrap();
        let left: u16 = visible[..position]
            .iter()
            .copied()
            .map(|visible| self.panel_width(visible))
            .sum::<u16>();
        let fixed_right: u16 = visible[position + 1..]
            .iter()
            .copied()
            .map(|visible| self.panel_width(visible))
            .sum::<u16>();
        let max = body_w.saturating_sub(left + fixed_right + MIN_TERM_W);
        if max < MIN_PANEL_W {
            return; // terminal too small to honor the minimums
        }
        self.set_panel_width(idx, want.saturating_sub(left).clamp(MIN_PANEL_W, max));
    }

    /// Re-fit panel widths to the current body width, shrinking the rightmost
    /// panel first, each floored at `MIN_PANEL_W`. Keeps the terminal pane at
    /// `MIN_TERM_W` whenever the screen allows it at all. The Workspaces bar
    /// spans the full width above them, so it costs the panels nothing here.
    pub fn normalize_panel_widths(&mut self, body_w: u16) {
        let budget = body_w.saturating_sub(MIN_TERM_W);
        let visible = self.visible_panel_indices();
        for i in visible.iter().rev().copied() {
            let others: u16 = visible
                .iter()
                .copied()
                .filter(|j| *j != i)
                .map(|j| self.panel_width(j))
                .sum::<u16>();
            let max = budget.saturating_sub(others);
            self.set_panel_width(
                i,
                self.panel_width(i).clamp(MIN_PANEL_W, max.max(MIN_PANEL_W)),
            );
        }
    }

    pub fn panel_width(&self, idx: usize) -> u16 {
        if idx == 3 {
            self.workflows.width
        } else {
            self.panel_widths[idx]
        }
    }

    fn set_panel_width(&mut self, idx: usize, width: u16) {
        if idx == 3 {
            self.workflows.width = width;
        } else {
            self.panel_widths[idx] = width;
        }
    }
}
