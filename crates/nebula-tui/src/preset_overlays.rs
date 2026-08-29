//! The AGENT PRESETS overlays: the list `e` opens in the SESSIONS PANEL and
//! the PRESET EDITOR form behind its `a` / `e` — their state, keys, mouse
//! and drawing. The presets themselves (and their file) are
//! `crate::agent_presets`; the task prompt a launch opens is an ordinary
//! multi-line `PromptDialog` (`PromptKind::AgentPresetTask`), and the
//! create it ends in goes through `event_loop::create_agent` like every
//! other session.

use crate::agent_presets::AgentPreset;
use crate::app::{
    clamp_selection, window_start, App, ConfirmDialog, Focus, Overlay, PendingAction, PromptKind,
};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect, empty_list_row, input_spans, modal_block, multiline_input_lines, render_row,
    row_rect, truncate,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::{AgentKind, WorktreeId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

const AGENT_PRESETS_W: u16 = 72;
/// The PRESET EDITOR's width and the tallest it draws (four single rows, a
/// blank, and two 4-row prefix/postfix boxes with their borders).
const PRESET_EDITOR_W: u16 = 76;
const PRESET_EDITOR_H: u16 = 19;

/// The AGENT PRESETS list (`e` in the SESSIONS PANEL): every saved preset,
/// snapshot from the store when the modal opens. Enter launches the
/// selected one into `worktree` after asking for a task; `a` / `e` / `d`
/// create, edit and delete.
#[derive(Debug, Clone)]
pub struct AgentPresetsView {
    pub presets: Vec<AgentPreset>,
    /// Cursor into `presets`.
    pub selected: usize,
    /// The WORKTREE a launch lands in — the one selected when `e` was
    /// pressed, carried so the editor and the delete confirm can reopen the
    /// list for the same target.
    pub worktree: WorktreeId,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the preset rows, written back during draw so clicks
    /// can hit-test rows.
    pub list_area: Rect,
}

impl AgentPresetsView {
    pub fn new(worktree: WorktreeId, presets: Vec<AgentPreset>) -> Self {
        Self {
            presets,
            selected: 0,
            worktree,
            area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }
}

/// One field of the PRESET EDITOR, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetField {
    Name,
    Kind,
    Model,
    Effort,
    Prefix,
    Postfix,
}

use crate::config::fits;

impl PresetField {
    pub const ALL: [PresetField; 6] = [
        PresetField::Name,
        PresetField::Kind,
        PresetField::Model,
        PresetField::Effort,
        PresetField::Prefix,
        PresetField::Postfix,
    ];

    /// The field `delta` steps away in Tab order, wrapping, and skipping
    /// an Effort the (kind, model) pair has no choice for — a Cursor
    /// family without effort variants, or no Cursor model yet.
    pub fn step(self, kind: AgentKind, model: &str, delta: i32) -> PresetField {
        let n = Self::ALL.len() as i32;
        let mut pos = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as i32;
        for _ in 0..n {
            pos = (pos + delta).rem_euclid(n);
            let next = Self::ALL[pos as usize];
            if next.available(kind, model) {
                return next;
            }
        }
        self
    }

    /// Whether the field applies to `kind` (with `model` chosen) at all.
    pub fn available(self, kind: AgentKind, model: &str) -> bool {
        match self {
            PresetField::Model => !crate::config::model_choices(kind).is_empty(),
            PresetField::Effort => !crate::config::effort_choices(kind, Some(model)).is_empty(),
            _ => true,
        }
    }

    /// Name, Prefix and Postfix take typed text; the rest cycle a choice.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            PresetField::Name | PresetField::Prefix | PresetField::Postfix
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            PresetField::Name => "Name",
            PresetField::Kind => "Harness",
            PresetField::Model => "Model",
            PresetField::Effort => "Effort",
            PresetField::Prefix => "Prefix",
            PresetField::Postfix => "Postfix",
        }
    }
}

/// The PRESET EDITOR: one form for creating (`editing == None`) or editing
/// (`editing == Some(index)`) an AGENT PRESET. Model and effort hold a
/// choice label from `config::model_choices` / `effort_choices`, so
/// `DEFAULT_CHOICE` means "follow Settings → Agents" and becomes None on save.
#[derive(Debug, Clone)]
pub struct AgentPresetEditor {
    /// Where the list this form came from launches into; Esc and save
    /// reopen it for the same target.
    pub worktree: WorktreeId,
    /// Index into the stored list when editing; None when creating.
    pub editing: Option<usize>,
    pub name: TextInput,
    pub kind: AgentKind,
    pub model: String,
    pub effort: String,
    pub prefix: TextInput,
    pub postfix: TextInput,
    /// The field with the caret / cycle focus.
    pub field: PresetField,
    /// Type-ahead on the focused choice row (Harness / Model / Effort): the
    /// letters typed so far; the row jumps to the best fuzzy match and ←/→
    /// cycle the matches only. Cleared on leaving the row.
    pub filter: String,
    /// Whole modal rect, written back during draw so a click outside can
    /// back out like Esc.
    pub area: Rect,
}

impl AgentPresetEditor {
    /// A blank form: Claude at the configured defaults, caret on the name.
    pub fn new(worktree: WorktreeId) -> Self {
        Self {
            worktree,
            editing: None,
            name: TextInput::new(),
            kind: AgentKind::Claude,
            model: crate::config::DEFAULT_CHOICE.into(),
            effort: crate::config::DEFAULT_CHOICE.into(),
            prefix: TextInput::new(),
            postfix: TextInput::new(),
            field: PresetField::Name,
            area: Rect::default(),
            filter: String::new(),
        }
    }

    /// The form pre-filled from a stored preset.
    pub fn from_preset(worktree: WorktreeId, index: usize, preset: &AgentPreset) -> Self {
        let choice = |v: &Option<String>| {
            v.clone()
                .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into())
        };
        Self {
            worktree,
            editing: Some(index),
            name: TextInput::with_text(preset.name.clone()),
            kind: preset.kind,
            model: choice(&preset.model),
            effort: choice(&preset.effort),
            prefix: TextInput::with_text(preset.prefix.clone()),
            postfix: TextInput::with_text(preset.postfix.clone()),
            field: PresetField::Name,
            area: Rect::default(),
            filter: String::new(),
        }
    }

    pub fn is_edit(&self) -> bool {
        self.editing.is_some()
    }

    /// Switch harness, dropping a model / effort the new kind doesn't list
    /// back to the default so the form never holds a choice it can't show.
    pub fn set_kind(&mut self, kind: AgentKind) {
        self.kind = kind;
        if !fits(&self.model, crate::config::model_choices(kind)) {
            self.model = crate::config::DEFAULT_CHOICE.into();
        }
        self.fit_effort();
        if !self.field.available(kind, &self.model) {
            self.field = PresetField::Prefix;
        }
    }

    /// Drop an effort the current (kind, model) pair doesn't list — Cursor's
    /// list follows the family — to what "default" would launch: the
    /// family's fallback for a Cursor family with no bare id, else default.
    fn fit_effort(&mut self) {
        let choices = crate::config::effort_choices(self.kind, Some(&self.model));
        if !fits(&self.effort, choices) {
            self.effort = crate::config::fit_effort(self.kind, Some(&self.model), None)
                .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into());
        }
    }

    /// The choices the focused row cycles: harness names, the kind's
    /// models, or the (kind, model) pair's efforts. Empty on a text row and
    /// on an `n/a` row.
    fn row_choices(&self) -> Vec<String> {
        match self.field {
            PresetField::Kind => AgentKind::ALL
                .iter()
                .map(|k| k.as_str().to_string())
                .collect(),
            PresetField::Model => crate::config::model_choices(self.kind)
                .iter()
                .map(|s| s.to_string())
                .collect(),
            PresetField::Effort => crate::config::effort_choices(self.kind, Some(&self.model))
                .iter()
                .map(|s| s.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn row_value(&self) -> String {
        match self.field {
            PresetField::Kind => self.kind.as_str().to_string(),
            PresetField::Model => self.model.clone(),
            PresetField::Effort => self.effort.clone(),
            _ => String::new(),
        }
    }

    fn set_row_value(&mut self, value: &str) {
        match self.field {
            PresetField::Kind => {
                if let Some(kind) = AgentKind::parse(value) {
                    self.set_kind(kind);
                }
            }
            PresetField::Model => {
                self.model = value.to_string();
                self.fit_effort();
            }
            PresetField::Effort => self.effort = value.to_string(),
            _ => {}
        }
    }

    /// The row's choices narrowed to the filter, best match first; all of
    /// them, in list order, while nothing is typed.
    pub fn filtered_choices(&self) -> Vec<String> {
        let all = self.row_choices();
        crate::fuzzy::rank(&self.filter, all.iter().map(String::as_str))
            .into_iter()
            .map(|(i, _)| all[i].clone())
            .collect()
    }

    /// A typed letter on a choice row: extend the filter and jump to its
    /// best match. False — and nothing changes — when no choice would
    /// match, so the row always shows something the filter names.
    pub fn type_filter(&mut self, c: char) -> bool {
        let query = format!("{}{c}", self.filter);
        let all = self.row_choices();
        let Some((best, _)) = crate::fuzzy::rank(&query, all.iter().map(String::as_str))
            .into_iter()
            .next()
        else {
            return false;
        };
        self.filter = query;
        let best = all[best].clone();
        self.set_row_value(&best);
        true
    }

    /// Backspace on a choice row: shorten the filter; the value stays
    /// unless it no longer matches, then the best match takes over.
    pub fn pop_filter(&mut self) {
        self.filter.pop();
        let matches = self.filtered_choices();
        let current = self.row_value();
        if !matches.iter().any(|m| m.eq_ignore_ascii_case(&current)) {
            if let Some(first) = matches.first().cloned() {
                self.set_row_value(&first);
            }
        }
    }

    /// `←` / `→` on a choice field: rotate it by `delta` — through the
    /// filter's matches while one is typed, else the whole list.
    pub fn cycle(&mut self, delta: i32) {
        if self.field.is_text() {
            return;
        }
        let choices = self.filtered_choices();
        if choices.is_empty() {
            return;
        }
        let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
        let next = crate::config::cycle_choice(&self.row_value(), &refs, delta).to_string();
        self.set_row_value(&next);
    }

    /// Shift+Enter / Ctrl+J in the prefix or postfix: a hard line break.
    pub fn prefix_or_postfix_newline(&mut self) {
        match self.field {
            PresetField::Prefix => self.prefix.insert_char('\n'),
            PresetField::Postfix => self.postfix.insert_char('\n'),
            _ => {}
        }
    }

    /// The text input under the caret, when the focused field is one.
    pub fn text_field_mut(&mut self) -> Option<&mut TextInput> {
        match self.field {
            PresetField::Name => Some(&mut self.name),
            PresetField::Prefix => Some(&mut self.prefix),
            PresetField::Postfix => Some(&mut self.postfix),
            _ => None,
        }
    }

    /// The preset the form describes right now (name trimmed, default
    /// choices folded to None).
    pub fn to_preset(&self) -> AgentPreset {
        AgentPreset {
            name: self.name.trim().to_string(),
            kind: self.kind,
            model: crate::config::non_default(&self.model),
            effort: crate::config::non_default(&self.effort),
            prefix: self.prefix.as_str().to_string(),
            postfix: self.postfix.as_str().to_string(),
        }
    }

    /// The preset to save, or why it can't be: a blank name, or one another
    /// preset (not the one being edited) already uses, case-insensitively.
    pub fn validate(&self, presets: &[AgentPreset]) -> Result<AgentPreset, String> {
        let preset = self.to_preset();
        if preset.name.is_empty() {
            return Err("the preset needs a name".into());
        }
        let taken = presets
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != self.editing && p.name.eq_ignore_ascii_case(&preset.name));
        if taken {
            return Err(format!("a preset named '{}' already exists", preset.name));
        }
        Ok(preset)
    }
}

/// `e` in the SESSIONS PANEL: the AGENT PRESETS list for the selected
/// WORKTREE — the one a launch lands in. Anywhere else the key just says
/// where it works, since without a worktree there is nothing to launch into.
pub(crate) fn open_agent_presets(app: &mut App) {
    let worktree = match (app.focus, app.selected_worktree()) {
        (Focus::Sessions, Some(w)) => w.id.clone(),
        _ => {
            app.flash = Some("agent presets: select a worktree in the Sessions panel first".into());
            return;
        }
    };
    reopen_agent_presets(app, worktree, 0);
}

/// (Re)open the AGENT PRESETS list from the store, cursor on `selected`
/// (clamped) — what the editor, the task editor's Esc and the delete
/// confirm come back to.
pub(crate) fn reopen_agent_presets(app: &mut App, worktree: WorktreeId, selected: usize) {
    let mut view = AgentPresetsView::new(worktree, crate::agent_presets::load());
    view.selected = clamp_selection(selected as i64, view.presets.len());
    app.overlay = Some(Overlay::AgentPresets(view));
}

/// The PRESET EDITOR: blank for `a`, pre-filled from the list's row for `e`.
pub(crate) fn open_agent_preset_editor(
    app: &mut App,
    worktree: WorktreeId,
    editing: Option<usize>,
) {
    let editor = match editing {
        Some(index) => match crate::agent_presets::load().get(index) {
            Some(preset) => AgentPresetEditor::from_preset(worktree, index, preset),
            None => {
                app.flash = Some("that preset is gone".into());
                reopen_agent_presets(app, worktree, 0);
                return;
            }
        },
        None => AgentPresetEditor::new(worktree),
    };
    app.overlay = Some(Overlay::AgentPresetEditor(editor));
}

/// Enter in the PRESET EDITOR: validate against the stored list, write the
/// row (in place when editing, appended when new), and land back on it in
/// the list. A rejected form stays open with the reason in the FOOTER.
pub(crate) fn save_agent_preset_editor(app: &mut App, editor: AgentPresetEditor) {
    let mut presets = crate::agent_presets::load();
    let preset = match editor.validate(&presets) {
        Ok(preset) => preset,
        Err(message) => {
            app.flash = Some(message);
            app.overlay = Some(Overlay::AgentPresetEditor(editor));
            return;
        }
    };
    let index = match editor.editing {
        Some(index) if index < presets.len() => {
            presets[index] = preset;
            index
        }
        _ => {
            presets.push(preset);
            presets.len() - 1
        }
    };
    if let Err(err) = crate::agent_presets::save(&presets) {
        app.flash = Some(format!("could not save agent presets: {err}"));
    }
    reopen_agent_presets(app, editor.worktree, index);
}

/// `d` in the AGENT PRESETS list: the confirm that guards the delete. The
/// list comes back either way (see `PendingAction::DeleteAgentPreset`).
pub(crate) fn open_delete_preset_confirm(app: &mut App, view: &AgentPresetsView) {
    let Some(preset) = view.presets.get(view.selected) else {
        return;
    };
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Delete preset".into(),
        message: format!(
            "Delete preset '{}'?\nIts saved prefix and postfix text go with it.",
            preset.name
        ),
        action: PendingAction::DeleteAgentPreset {
            index: view.selected,
            worktree: view.worktree.clone(),
        },
        area: ratatui::layout::Rect::default(),
    }));
}

/// The list's Enter: ask for the task that the preset's prefix and postfix
/// will wrap. A harness switched off in Settings → Agents is refused here,
/// where the row is, rather than by a failed spawn later.
pub(crate) fn open_agent_preset_task(app: &mut App, view: &AgentPresetsView) {
    let Some(preset) = view.presets.get(view.selected).cloned() else {
        app.flash = Some("no preset selected — a creates one".into());
        return;
    };
    if !crate::config::Config::load().kind_enabled(preset.kind) {
        app.flash = Some(format!(
            "{} is turned off in Settings → Agents",
            preset.kind.as_str()
        ));
        return;
    }
    crate::event_loop::open_prompt(
        app,
        PromptKind::AgentPresetTask {
            worktree: view.worktree.clone(),
            preset,
        },
    );
}

/// Keys in the AGENT PRESETS list.
pub(crate) fn handle_list_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.overlay = None,
        KeyCode::Char('j') | KeyCode::Down => {
            view.selected = clamp_selection(view.selected as i64 + 1, view.presets.len());
        }
        KeyCode::Char('k') | KeyCode::Up => {
            view.selected = clamp_selection(view.selected as i64 - 1, view.presets.len());
        }
        KeyCode::Char('a') | KeyCode::Char('n') => {
            let worktree = view.worktree.clone();
            open_agent_preset_editor(app, worktree, None);
        }
        KeyCode::Char('e') => {
            if view.presets.is_empty() {
                app.flash = Some("no preset selected — a creates one".into());
            } else {
                let (worktree, index) = (view.worktree.clone(), view.selected);
                open_agent_preset_editor(app, worktree, Some(index));
            }
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
            let view = view.clone();
            open_delete_preset_confirm(app, &view);
        }
        KeyCode::Enter => {
            let view = view.clone();
            open_agent_preset_task(app, &view);
        }
        _ => {}
    }
}

/// Keys in the PRESET EDITOR: Tab order between fields, choice cycling,
/// text editing, save and back.
pub(crate) fn handle_editor_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::AgentPresetEditor(editor)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let multiline = matches!(editor.field, PresetField::Prefix | PresetField::Postfix);
    match key.code {
        // Esc first clears a choice row's type-ahead, then backs out.
        KeyCode::Esc if !editor.filter.is_empty() => editor.filter.clear(),
        // Back to the list, unsaved.
        KeyCode::Esc => {
            let (worktree, index) = (editor.worktree.clone(), editor.editing.unwrap_or(0));
            reopen_agent_presets(app, worktree, index);
        }
        // Leaving a choice row drops its type-ahead.
        KeyCode::Tab | KeyCode::Down => {
            editor.filter.clear();
            editor.field = editor.field.step(editor.kind, &editor.model, 1)
        }
        KeyCode::BackTab | KeyCode::Up => {
            editor.filter.clear();
            editor.field = editor.field.step(editor.kind, &editor.model, -1)
        }
        // A hard line in the prefix / postfix, as in the task editor.
        KeyCode::Char('j') if multiline && ctrl => editor.prefix_or_postfix_newline(),
        KeyCode::Enter if multiline && shift => editor.prefix_or_postfix_newline(),
        KeyCode::Enter => {
            let editor = editor.clone();
            save_agent_preset_editor(app, editor);
        }
        KeyCode::Left if !editor.field.is_text() => editor.cycle(-1),
        KeyCode::Right | KeyCode::Char(' ') if !editor.field.is_text() => editor.cycle(1),
        // Letters on a choice row type ahead: the row jumps to the best
        // match and ←/→ cycle the matches. A letter nothing matches is
        // refused, so the row never shows a choice the filter denies.
        KeyCode::Backspace if !editor.field.is_text() => editor.pop_filter(),
        KeyCode::Char(c) if !editor.field.is_text() && !ctrl => {
            let query = format!("{}{c}", editor.filter);
            if !editor.type_filter(c) {
                app.flash = Some(format!("no choice matches '{query}'"));
            }
        }
        _ => {
            if let Some(input) = editor.text_field_mut() {
                input.handle_key(&key);
            }
        }
    }
}

/// Mouse in the AGENT PRESETS list: the wheel moves the selection, a click
/// on a row launches it (rows are actions, as in the hosts picker — editing
/// is `e`), a click outside the modal closes; everything else is swallowed.
pub(crate) fn handle_list_mouse(app: &mut App, mouse: MouseEvent, mouse_pos: Position) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            view.selected = clamp_selection(view.selected as i64 - 1, view.presets.len());
            app.dirty = true;
        }
        MouseEventKind::ScrollDown => {
            view.selected = clamp_selection(view.selected as i64 + 1, view.presets.len());
            app.dirty = true;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let list = view.list_area;
            let inside_list = list.contains(mouse_pos);
            let inside_modal = view.area.contains(mouse_pos);
            if inside_list {
                let start = view.window_start(list.height as usize);
                let index = start + (mouse.row - list.y) as usize;
                if index < view.presets.len() {
                    view.selected = index;
                    let view = view.clone();
                    open_agent_preset_task(app, &view);
                }
            } else if !inside_modal {
                app.overlay = None;
            }
            app.dirty = true;
        }
        _ => {}
    }
}

/// The AGENT PRESETS list modal.
pub(crate) fn draw_list(f: &mut Frame, app: &mut App, view: &AgentPresetsView, th: Theme) {
    let total = view.presets.len();
    let selected = view.selected.min(total.saturating_sub(1));
    let height = (total.max(1) as u16)
        .saturating_add(2)
        .clamp(5, f.area().height.max(5));
    let area = centered_rect(f.area(), AGENT_PRESETS_W, height);
    f.render_widget(Clear, area);
    let hint = " Enter: launch  a: new  e: edit  d: delete  Esc: close ";
    let block = modal_block(" Agent presets ", th)
        .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if total == 0 {
        empty_list_row(f, inner, "no presets yet — a creates one", th);
    }
    let start = view.window_start(inner.height as usize);
    for (i, preset) in view.presets.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(inner, i - start) else {
            break;
        };
        let budget = (inner.width as usize).saturating_sub(2);
        // "name  claude · opus · high" left, a dim "+prefix +postfix"
        // pinned right when the preset wraps the task.
        let mut marks = Vec::new();
        if !preset.prefix.trim().is_empty() {
            marks.push("+prefix");
        }
        if !preset.postfix.trim().is_empty() {
            marks.push("+postfix");
        }
        let marks = marks.join(" ");
        let marks_w = marks.chars().count();
        let text_budget = budget.saturating_sub(if marks_w > 0 { marks_w + 2 } else { 0 });
        let name_txt = truncate(&preset.name, text_budget);
        let mut used = name_txt.chars().count();
        let mut spans = vec![Span::raw(name_txt)];
        let spec = preset.spec_label();
        if used + 2 < text_budget {
            let spec = truncate(&format!("  {spec}"), text_budget - used);
            used += spec.chars().count();
            spans.push(Span::styled(spec, Style::default().fg(th.dim)));
        }
        if marks_w > 0 && used + marks_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - marks_w)));
            spans.push(Span::styled(marks, Style::default().fg(th.dim)));
        }
        render_row(f, row_area, spans, i == selected, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing, plus the clamped cursor.
    if let Some(Overlay::AgentPresets(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = inner;
        v.selected = selected;
    }
}

/// The PRESET EDITOR form.
pub(crate) fn draw_editor(f: &mut Frame, app: &mut App, editor: &AgentPresetEditor, th: Theme) {
    // Four single rows, a blank, then the two text boxes; the boxes
    // give up rows first on a short screen, down to one line each.
    let frame_h = f.area().height.max(9);
    let box_h = ((frame_h.min(PRESET_EDITOR_H).saturating_sub(7)) / 2).clamp(3, 6);
    let height = 7 + 2 * box_h;
    let area = centered_rect(f.area(), PRESET_EDITOR_W, height);
    f.render_widget(Clear, area);
    let hint = if area.width >= 72 {
        " Tab/↑↓: field  ←/→ or type: choose  ⇧Enter: newline  Enter: save  Esc "
    } else {
        " Tab: field  ←/→ or type: choose  Enter: save  Esc "
    };
    let title = if editor.is_edit() {
        format!(" Edit preset — {} ", editor.name.trim())
    } else {
        " New preset ".to_string()
    };
    let block = modal_block(title, th)
        .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let label_w = 9usize;
    let single = [
        PresetField::Name,
        PresetField::Kind,
        PresetField::Model,
        PresetField::Effort,
    ];
    for (i, field) in single.iter().enumerate() {
        let Some(row_area) = row_rect(inner, i) else {
            break;
        };
        let focused = editor.field == *field;
        let available = field.available(editor.kind, &editor.model);
        let label_style = if focused {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else if available {
            Style::default().fg(th.muted)
        } else {
            Style::default().fg(th.dim)
        };
        let mut spans = vec![Span::styled(
            format!("{:>label_w$}  ", field.label()),
            label_style,
        )];
        let budget = (inner.width as usize).saturating_sub(2 + label_w + 2);
        match field {
            PresetField::Name => {
                if focused {
                    spans.extend(input_spans(&editor.name, budget, th.accent, th));
                } else if editor.name.trim().is_empty() {
                    spans.push(Span::styled("(required)", Style::default().fg(th.dim)));
                } else {
                    spans.push(Span::raw(truncate(editor.name.as_str(), budget)));
                }
            }
            _ => {
                let value = match field {
                    PresetField::Kind => editor.kind.as_str().to_string(),
                    PresetField::Model => editor.model.clone(),
                    PresetField::Effort => editor.effort.clone(),
                    _ => String::new(),
                };
                if !available {
                    spans.push(Span::styled("n/a", Style::default().fg(th.dim)));
                } else if focused {
                    spans.push(Span::styled("◂ ", Style::default().fg(th.accent)));
                    spans.push(Span::styled(
                        value,
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(" ▸", Style::default().fg(th.accent)));
                    if !editor.filter.is_empty() {
                        spans.push(Span::styled(
                            format!("  ⌕ {}", editor.filter),
                            Style::default().fg(th.accent),
                        ));
                        spans.push(Span::styled(
                            format!(" ({})", editor.filtered_choices().len()),
                            Style::default().fg(th.dim),
                        ));
                    }
                } else {
                    spans.push(Span::raw(value));
                }
            }
        }
        render_row(f, row_area, spans, focused, true, th);
    }

    // The prefix / postfix boxes, stacked under a blank row.
    let boxes = [
        (
            PresetField::Prefix,
            &editor.prefix,
            " Prefix (optional) ",
            "sent before your task",
        ),
        (
            PresetField::Postfix,
            &editor.postfix,
            " Postfix (optional) ",
            "sent after your task",
        ),
    ];
    for (n, (field, input, title, placeholder)) in boxes.iter().enumerate() {
        let y = inner.y.saturating_add(5 + n as u16 * box_h);
        if y + box_h > inner.y + inner.height {
            break;
        }
        let box_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: box_h,
        };
        let focused = editor.field == *field;
        let border = if focused { th.accent } else { th.dim };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .title(Span::styled(*title, Style::default().fg(border)));
        let box_inner = block.inner(box_area);
        f.render_widget(block, box_area);
        if focused {
            let (lines, caret_row) =
                multiline_input_lines(input, box_inner.width as usize, th.accent, th);
            let visible = box_inner.height.max(1) as usize;
            let max_start = lines.len().saturating_sub(visible);
            let start = caret_row.saturating_sub(visible / 2).min(max_start);
            let shown: Vec<Line> = lines.into_iter().skip(start).take(visible).collect();
            f.render_widget(Paragraph::new(shown), box_inner);
        } else if input.trim().is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(*placeholder, Style::default().fg(th.dim))),
                box_inner,
            );
        } else {
            f.render_widget(
                Paragraph::new(input.as_str().to_string())
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                box_inner,
            );
        }
    }

    // Write-back (draw works on a clone): the rect a click outside
    // of backs out from.
    if let Some(Overlay::AgentPresetEditor(e)) = &mut app.overlay {
        e.area = area;
    }
}
