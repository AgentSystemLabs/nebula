//! The AGENT PRESETS overlays: the list `e` opens in the SESSIONS PANEL
//! and on a checkout's row of the WORKTREES PANEL
//! (and, as a picker, on a PROJECT OPEN PRS GROUP row of the WORKTREES
//! PANEL — a PR SESSION on that pull request, through the QUICK PROMPT)
//! and the PRESET EDITOR form behind its `Ctrl+a` / `Ctrl+e` — their
//! state, keys, mouse and drawing. The form's Text row picks which side of
//! the task the preset's text goes (`agent_presets::PresetText`) — a prefix
//! box, a postfix box or both — starting on **Preset text** (Settings →
//! Sessions) for a new preset and on every side a stored one holds text
//! for; a side the row leaves out saves blank. The presets themselves (and
//! their file) are
//! `crate::agent_presets`; the task prompt a launch opens is an ordinary
//! multi-line `PromptDialog` (`PromptKind::AgentPresetTask`) — sent on at
//! once, empty, for a `skip_task` preset (`event_loop::submit_prompt_now`)
//! — and the create it ends in goes through `event_loop::create_agent`
//! like every other session.

use crate::agent_presets::{AgentPreset, PresetText};
use crate::app::{
    clamp_selection, window_start, App, ConfirmDialog, Focus, Overlay, PendingAction, PromptKind,
};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect, draw_multiline_input, draw_scroll_marks, empty_list_row, fuzzy_highlight_spans,
    input_spans, modal_block, render_row, row_rect, truncate, visible_positions,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nebula_core::{AgentKind, ClientRequest, WorktreeId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

const AGENT_PRESETS_W: u16 = 72;
/// The PRESET EDITOR's width and the tallest it draws (five single rows, a
/// blank, the text boxes — two 4-row prefix / postfix boxes with their
/// borders, or one twice as tall — and the Task row under them) — one row
/// more while a refused save's banner is up.
const PRESET_EDITOR_W: u16 = 76;
const PRESET_EDITOR_H: u16 = 21;
/// The Task row's two choices: ask for an (optional) task on launch, or
/// launch at once (`AgentPreset::skip_task`).
const TASK_ASK: &str = "ask";
const TASK_SKIP: &str = "skip";

/// The AGENT PRESETS list (`e` in the SESSIONS PANEL): every saved preset,
/// snapshot from the store when the modal opens. Letters type ahead over
/// the names (`filter`), as in the MODEL / EFFORT submenus. Enter launches
/// the selected one into `worktree` after asking for an optional task — at
/// once for a `skip_task` preset; `Ctrl+a` / `Ctrl+e` / `Ctrl+d` create,
/// edit and delete.
#[derive(Debug, Clone)]
pub struct AgentPresetsView {
    pub presets: Vec<AgentPreset>,
    /// Cursor into `presets` — the stored index, whatever the filter shows,
    /// so the editor, the delete confirm and the launch read the right row.
    pub selected: usize,
    /// The type-ahead typed so far: the rows narrow to the names it fuzzy
    /// matches, best match first (`visible`). Empty shows every preset in
    /// its stored order.
    pub filter: String,
    /// The WORKTREE a launch lands in — the one selected when `e` was
    /// pressed, carried so the editor and the delete confirm can reopen the
    /// list for the same target.
    pub worktree: WorktreeId,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the preset rows, written back during draw so clicks
    /// can hit-test rows.
    pub list_area: Rect,
    /// Set when the list was opened as a QUICK PROMPT picker (`Shift+Tab`
    /// in the box, `e` on a PROJECT OPEN PRS GROUP row): the box to put
    /// back, with the text typed so far. In that mode Enter applies the
    /// row to that launch and Esc returns unchanged; `Ctrl+a` / `Ctrl+e` /
    /// `Ctrl+d` manage the presets exactly as in the manager, and the
    /// editor and the delete confirm carry this along so both come back to
    /// the picker, not to a manager launching into `worktree`.
    pub quick: Option<crate::quick_prompt::QuickReturn>,
}

impl AgentPresetsView {
    pub fn new(worktree: WorktreeId, presets: Vec<AgentPreset>) -> Self {
        Self {
            presets,
            selected: 0,
            filter: String::new(),
            worktree,
            area: Rect::default(),
            list_area: Rect::default(),
            quick: None,
        }
    }

    /// Is this list a QUICK PROMPT picker rather than the manager?
    pub fn is_picker(&self) -> bool {
        self.quick.is_some()
    }

    /// The box Esc puts back: only a picker the box itself opened has one
    /// (`QuickReturn::from_box`).
    pub fn box_behind(&self) -> Option<&crate::quick_prompt::QuickReturn> {
        self.quick.as_ref().filter(|q| q.from_box)
    }

    /// The rows `filter` leaves, top to bottom: indices into `presets`,
    /// each with the name's matched char positions (lit when drawn). Worked
    /// out afresh on every call rather than kept — a handful of names — so
    /// it can never go stale against `presets`.
    pub fn visible(&self) -> Vec<(usize, Vec<usize>)> {
        crate::fuzzy::rank(&self.filter, self.presets.iter().map(|p| p.name.as_str()))
    }

    /// Where the cursor sits among the visible rows: the top one when the
    /// filter hides `selected`, the last when an unfiltered `selected` has
    /// run past the end.
    fn cursor(&self, visible: &[(usize, Vec<usize>)]) -> usize {
        visible
            .iter()
            .position(|(i, _)| *i == self.selected)
            .unwrap_or(if self.filter.is_empty() {
                self.selected.min(visible.len().saturating_sub(1))
            } else {
                0
            })
    }

    /// ↑/↓: move `delta` rows through the visible ones.
    pub fn step(&mut self, delta: i64) {
        let visible = self.visible();
        let at = clamp_selection(self.cursor(&visible) as i64 + delta, visible.len());
        if let Some((index, _)) = visible.get(at) {
            self.selected = *index;
        }
    }

    /// The row `pos` rows down the visible list (a click, a wheel), as an
    /// index into `presets`.
    pub fn visible_row(&self, pos: usize) -> Option<usize> {
        self.visible().get(pos).map(|(index, _)| *index)
    }

    /// Narrow to `query`, the cursor on its best match. False — and
    /// nothing changes — when no name matches, so the list never empties
    /// under the user (as in the MODEL / EFFORT submenus). An empty query
    /// shows every row and leaves the cursor where it is: the preset just
    /// found stays selected once Esc clears the letters that found it.
    pub fn set_filter(&mut self, query: &str) -> bool {
        let ranked = crate::fuzzy::rank(query, self.presets.iter().map(|p| p.name.as_str()));
        let Some((best, _)) = ranked.first() else {
            return false;
        };
        if !query.trim().is_empty() {
            self.selected = *best;
        }
        self.filter = query.to_string();
        true
    }

    /// One typed character onto the filter; false when it would leave no
    /// rows (nothing changes).
    pub fn type_filter(&mut self, c: char) -> bool {
        self.set_filter(&format!("{}{c}", self.filter))
    }

    /// Backspace: drop the last character (widens, so it always succeeds).
    pub fn pop_filter(&mut self) {
        let mut query = self.filter.clone();
        query.pop();
        self.set_filter(&query);
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor(&self.visible()), height)
    }
}

/// One field of the PRESET EDITOR, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetField {
    Name,
    Kind,
    Model,
    Effort,
    /// Which side(s) of the task the text goes: the boxes below.
    Text,
    Prefix,
    Postfix,
    Task,
}

use crate::config::fits;

impl PresetField {
    pub const ALL: [PresetField; 8] = [
        PresetField::Name,
        PresetField::Kind,
        PresetField::Model,
        PresetField::Effort,
        PresetField::Text,
        PresetField::Prefix,
        PresetField::Postfix,
        PresetField::Task,
    ];

    /// The field `delta` steps away in Tab order, wrapping, and skipping
    /// what the form has no row for: an Effort the (harness, model) pair
    /// has no choice for — a Cursor family without effort variants, or no
    /// Cursor model yet — and the box on a side the Text row leaves out.
    pub fn step(self, editor: &AgentPresetEditor, delta: i32) -> PresetField {
        let n = Self::ALL.len() as i32;
        let mut pos = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as i32;
        for _ in 0..n {
            pos = (pos + delta).rem_euclid(n);
            let next = Self::ALL[pos as usize];
            if next.available(editor) {
                return next;
            }
        }
        self
    }

    /// Whether the form shows the field at all: Model and Effort follow
    /// the harness (with its model chosen), Prefix and Postfix the Text
    /// row.
    pub fn available(self, editor: &AgentPresetEditor) -> bool {
        let (kind, custom) = (editor.kind, editor.custom.as_deref());
        match self {
            PresetField::Model => !crate::config::model_choices(kind, custom).is_empty(),
            PresetField::Effort => {
                !crate::config::effort_choices(kind, Some(&editor.model), custom).is_empty()
            }
            PresetField::Prefix => editor.text.has_prefix(),
            PresetField::Postfix => editor.text.has_postfix(),
            _ => true,
        }
    }

    /// Name, Prefix and Postfix take typed text; the rest — the Text row
    /// among them — cycle a choice.
    pub fn is_typed(self) -> bool {
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
            PresetField::Text => "Text",
            PresetField::Prefix => "Prefix",
            PresetField::Postfix => "Postfix",
            PresetField::Task => "Task",
        }
    }
}

/// Why the last Enter in the PRESET EDITOR saved nothing: the message its
/// banner shows, and the field the banner points at — drawn in the error
/// color, with the caret moved onto it, until its text changes. Only the
/// Name row can fail today (`AgentPresetEditor::validate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormError {
    pub field: PresetField,
    pub message: String,
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
    /// The QUICK PROMPT box behind the list, when the form was opened from
    /// a picker (`AgentPresetsView::quick`): Esc and save reopen the list
    /// as that picker, so the box, its text and its pull request survive.
    pub quick: Option<crate::quick_prompt::QuickReturn>,
    /// Index into the stored list when editing; None when creating.
    pub editing: Option<usize>,
    pub name: TextInput,
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
    pub model: String,
    pub effort: String,
    /// Which side(s) of the task the preset's text goes — the boxes the
    /// form shows and the sides `to_preset` keeps. A hidden box keeps its
    /// text while the form is open (cycling back shows it again) and
    /// saves blank, so the form never saves what it doesn't show.
    pub text: PresetText,
    pub prefix: TextInput,
    pub postfix: TextInput,
    /// Launch without asking for a task (`AgentPreset::skip_task`); the
    /// Task row shows it as `ask` / `skip`.
    pub skip_task: bool,
    /// The field with the caret / cycle focus.
    pub field: PresetField,
    /// Type-ahead on the focused choice row (Harness / Model / Effort): the
    /// letters typed so far; the row jumps to the best fuzzy match and ←/→
    /// cycle the matches only. Cleared on leaving the row.
    pub filter: String,
    /// Why the last Enter was refused, while it still applies: the banner
    /// at the top of the form and the field it colors. Cleared by the
    /// first change to that field's text — the fix is under way, and the
    /// next Enter judges the new text afresh.
    pub error: Option<FormError>,
    /// Whole modal rect, written back during draw so a click outside can
    /// back out like Esc.
    pub area: Rect,
}

impl AgentPresetEditor {
    /// A blank form: Claude at the configured defaults, the box(es) the
    /// **Preset text** setting names (`text`), caret on the name.
    pub fn new(worktree: WorktreeId, text: PresetText) -> Self {
        Self {
            worktree,
            quick: None,
            editing: None,
            name: TextInput::new(),
            kind: AgentKind::Claude,
            custom: None,
            model: crate::config::DEFAULT_CHOICE.into(),
            effort: crate::config::DEFAULT_CHOICE.into(),
            text,
            prefix: TextInput::multiline(),
            postfix: TextInput::multiline(),
            skip_task: false,
            field: PresetField::Name,
            area: Rect::default(),
            filter: String::new(),
            error: None,
        }
    }

    /// The form pre-filled from a stored preset, showing every side it
    /// holds text on as well as the side(s) the **Preset text** setting
    /// (`default`) names — nothing saved is hidden.
    pub fn from_preset(
        worktree: WorktreeId,
        index: usize,
        preset: &AgentPreset,
        default: PresetText,
    ) -> Self {
        let choice = |v: &Option<String>| {
            v.clone()
                .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into())
        };
        Self {
            worktree,
            quick: None,
            editing: Some(index),
            name: TextInput::with_text(preset.name.clone()),
            kind: preset.kind,
            custom: preset.custom_harness.clone(),
            model: choice(&preset.model),
            effort: choice(&preset.effort),
            text: PresetText::for_preset(preset, default),
            prefix: TextInput::multiline_with_text(preset.prefix.clone()),
            postfix: TextInput::multiline_with_text(preset.postfix.clone()),
            skip_task: preset.skip_task,
            field: PresetField::Name,
            area: Rect::default(),
            filter: String::new(),
            error: None,
        }
    }

    pub fn is_edit(&self) -> bool {
        self.editing.is_some()
    }

    /// The Task row's value: `ask` or `skip`.
    fn task_choice(&self) -> &'static str {
        if self.skip_task {
            TASK_SKIP
        } else {
            TASK_ASK
        }
    }

    /// Switch harness, dropping a model / effort the new kind doesn't list
    /// back to the default so the form never holds a choice it can't show.
    pub fn set_kind(&mut self, kind: AgentKind, custom: Option<String>) {
        self.kind = kind;
        self.custom = custom;
        if !fits(
            &self.model,
            &crate::config::model_choices(kind, self.custom.as_deref()),
        ) {
            self.model = crate::config::DEFAULT_CHOICE.into();
        }
        self.fit_effort();
        if !self.field.available(self) {
            self.field = PresetField::Text;
        }
    }

    /// Drop an effort the current (harness, model) pair doesn't list — a
    /// composing harness's list follows the family — to what "default"
    /// would launch: the family's fallback for a family with no bare id,
    /// else default.
    fn fit_effort(&mut self) {
        let choices =
            crate::config::effort_choices(self.kind, Some(&self.model), self.custom.as_deref());
        if !fits(&self.effort, &choices) {
            self.effort = crate::config::fit_effort(
                self.kind,
                Some(&self.model),
                None,
                self.custom.as_deref(),
            )
            .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into());
        }
    }

    /// The choices the focused row cycles: harness names, the kind's
    /// models, the (kind, model) pair's efforts, the Text row's sides, or
    /// `ask` / `skip`. Empty on a typed row and on an `n/a` row.
    fn row_choices(&self) -> Vec<String> {
        match self.field {
            // Every offered harness by id — built-ins and customs alike,
            // in registry order. Never a bare `custom`, which carries no
            // entry.
            PresetField::Kind => crate::config::Config::load()
                .offered_harnesses()
                .into_iter()
                .map(|(kind, custom)| custom.unwrap_or_else(|| kind.as_str().to_string()))
                .collect(),
            PresetField::Model => crate::config::model_choices(self.kind, self.custom.as_deref()),
            PresetField::Effort => {
                crate::config::effort_choices(self.kind, Some(&self.model), self.custom.as_deref())
            }
            PresetField::Text => PresetText::ALL
                .iter()
                .map(|side| side.as_str().to_string())
                .collect(),
            PresetField::Task => vec![TASK_ASK.to_string(), TASK_SKIP.to_string()],
            _ => Vec::new(),
        }
    }

    fn row_value(&self) -> String {
        match self.field {
            PresetField::Kind => self
                .custom
                .as_deref()
                .unwrap_or_else(|| self.kind.as_str())
                .to_string(),
            PresetField::Model => self.model.clone(),
            PresetField::Effort => self.effort.clone(),
            PresetField::Text => self.text.as_str().to_string(),
            PresetField::Task => self.task_choice().to_string(),
            _ => String::new(),
        }
    }

    fn set_row_value(&mut self, value: &str) {
        match self.field {
            PresetField::Kind => {
                if let Some(kind) = AgentKind::parse(value) {
                    self.set_kind(kind, None);
                } else if crate::config::Config::load()
                    .harness_registry()
                    .iter()
                    .any(|entry| entry.id == value)
                {
                    self.set_kind(AgentKind::Custom, Some(value.to_string()));
                }
            }
            PresetField::Model => {
                self.model = value.to_string();
                self.fit_effort();
            }
            PresetField::Effort => self.effort = value.to_string(),
            PresetField::Text => {
                if let Some(side) = PresetText::parse(value) {
                    self.text = side;
                }
            }
            PresetField::Task => self.skip_task = value == TASK_SKIP,
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
        if self.field.is_typed() {
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

    /// The text input under the caret, when the focused field is one.
    pub fn text_field(&self) -> Option<&TextInput> {
        match self.field {
            PresetField::Name => Some(&self.name),
            PresetField::Prefix => Some(&self.prefix),
            PresetField::Postfix => Some(&self.postfix),
            _ => None,
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

    /// A key on the focused text field. A change to the field the banner
    /// points at takes the banner down: the fix is under way.
    pub fn edit_text(&mut self, key: &KeyEvent) {
        let field = self.field;
        let Some(input) = self.text_field_mut() else {
            return;
        };
        if input.handle_key(key).changed() {
            self.text_changed(field);
        }
    }

    /// A bracketed paste into the focused text field — lines kept in the
    /// prefix / postfix, flattened in the one-line name — clearing a banner
    /// that points at it, as a typed key does. False when a choice row has
    /// the caret and there is nowhere to paste.
    pub fn paste(&mut self, text: &str) -> bool {
        let field = self.field;
        let Some(input) = self.text_field_mut() else {
            return false;
        };
        input.insert_str(text);
        self.text_changed(field);
        true
    }

    /// Enter was refused: keep the reason for the banner, and put the
    /// caret on the field it names — wherever it was — so the next
    /// keystroke is the fix.
    pub fn reject(&mut self, error: FormError) {
        self.filter.clear();
        self.field = error.field;
        self.error = Some(error);
    }

    /// The text of `field` changed: a banner pointing at it comes down.
    /// One pointing elsewhere stays — that field is still the problem.
    fn text_changed(&mut self, field: PresetField) {
        if self.error.as_ref().is_some_and(|e| e.field == field) {
            self.error = None;
        }
    }

    /// The preset the form describes right now (name trimmed, default
    /// choices folded to None, a side the Text row leaves out blank).
    pub fn to_preset(&self) -> AgentPreset {
        // A hidden box saves empty: the form never saves what it doesn't
        // show. Its text waits in the form until then.
        let side = |shown: bool, input: &TextInput| {
            if shown {
                input.as_str().to_string()
            } else {
                String::new()
            }
        };
        AgentPreset {
            name: self.name.trim().to_string(),
            kind: self.kind,
            custom_harness: self.custom.clone(),
            model: crate::config::non_default(&self.model),
            effort: crate::config::non_default(&self.effort),
            prefix: side(self.text.has_prefix(), &self.prefix),
            postfix: side(self.text.has_postfix(), &self.postfix),
            skip_task: self.skip_task,
        }
    }

    /// The preset to save, or why it can't be — pointing at the Name row:
    /// blank, or a name another preset (not the one being edited) already
    /// uses, case-insensitively.
    pub fn validate(&self, presets: &[AgentPreset]) -> Result<AgentPreset, FormError> {
        let preset = self.to_preset();
        let name_error = |message: String| FormError {
            field: PresetField::Name,
            message,
        };
        if preset.name.is_empty() {
            return Err(name_error("the preset needs a name".into()));
        }
        let taken = presets
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != self.editing && p.name.eq_ignore_ascii_case(&preset.name));
        if taken {
            return Err(name_error(format!(
                "a preset named '{}' already exists",
                preset.name
            )));
        }
        Ok(preset)
    }
}

/// `e` in the SESSIONS PANEL, or on a checkout's row of the WORKTREES
/// PANEL: the AGENT PRESETS list for the selected WORKTREE — the one a
/// launch lands in. The two panels answer alike because both cursors name
/// that checkout: the Worktrees one is on it, the Sessions one is inside
/// it. (`p` in the WORKTREES PANEL cuts a fresh worktree first; a preset
/// runs in the checkout it was reached from.) With the Worktrees cursor on
/// a PROJECT OPEN PRS GROUP row it is the same list as a picker for a PR
/// SESSION on that pull request (`quick_prompt::open_preset_picker_for_pr`)
/// — from whichever panel has FOCUS, as `p` is: a pull request row has no
/// worktree and no sessions to manage presets against, so the pull
/// request is the only thing the key can be for, and the pane reading it
/// is where a review preset is most often reached for. Anywhere else the
/// key just says where it works, since without a worktree or a pull
/// request there is nothing to launch into.
pub(crate) fn open_agent_presets(app: &mut App) {
    if app.selected_worktree_pr().is_some() {
        crate::quick_prompt::open_preset_picker_for_pr(app);
        return;
    }
    // An issue row the same way: the preset launches on the issue.
    if app.selected_worktree_issue().is_some() {
        crate::issues::open_preset_for_row(app);
        return;
    }
    let worktree = match (app.focus, app.selected_worktree()) {
        (Focus::Sessions | Focus::Worktrees, Some(w)) => w.id.clone(),
        _ => {
            app.flash = Some(
                "agent presets: select a worktree or an open PR in the Worktrees panel, or a worktree's Sessions panel"
                    .into(),
            );
            return;
        }
    };
    reopen_agent_presets(app, worktree, 0);
}

/// (Re)open the AGENT PRESETS list from the store, cursor on `selected`
/// (clamped) — what the editor, the task editor's Esc and the delete
/// confirm come back to.
pub(crate) fn reopen_agent_presets(app: &mut App, worktree: WorktreeId, selected: usize) {
    reopen_presets_list(app, worktree, None, selected);
}

/// (Re)open the list the editor or the delete confirm was reached from:
/// the manager, or — with `quick` — the QUICK PROMPT picker for that box,
/// so a preset added, edited or deleted from a picker lands back in the
/// picker with the box, its text and its pull request intact.
pub(crate) fn reopen_presets_list(
    app: &mut App,
    worktree: WorktreeId,
    quick: Option<crate::quick_prompt::QuickReturn>,
    selected: usize,
) {
    let mut view = AgentPresetsView::new(worktree, crate::agent_presets::load());
    view.selected = clamp_selection(selected as i64, view.presets.len());
    view.quick = quick;
    app.overlay = Some(Overlay::AgentPresets(view));
}

/// The PRESET EDITOR: blank for `Ctrl+a`, pre-filled from the list's row
/// for `Ctrl+e` — from the manager or a picker alike; `quick` is the
/// picker's box, to come back to (`AgentPresetEditor::quick`). Its boxes
/// are the side(s) **Preset text** (Settings → Sessions) names, plus any a
/// stored preset already holds text on.
pub(crate) fn open_agent_preset_editor(
    app: &mut App,
    worktree: WorktreeId,
    quick: Option<crate::quick_prompt::QuickReturn>,
    editing: Option<usize>,
) {
    let text = crate::config::Config::load().preset_text();
    let mut editor = match editing {
        Some(index) => match crate::agent_presets::load().get(index) {
            Some(preset) => AgentPresetEditor::from_preset(worktree, index, preset, text),
            None => {
                app.flash = Some("that preset is gone".into());
                reopen_presets_list(app, worktree, quick, 0);
                return;
            }
        },
        None => AgentPresetEditor::new(worktree, text),
    };
    editor.quick = quick;
    app.overlay = Some(Overlay::AgentPresetEditor(editor));
}

/// Enter in the PRESET EDITOR: validate against the stored list, write the
/// row (in place when editing, appended when new), and land back on it in
/// the list. A rejected form stays open, the reason on a banner at the top
/// of it and the caret on the field to fix (`AgentPresetEditor::reject`) —
/// not in the FOOTER, which is nowhere near the form.
pub(crate) fn save_agent_preset_editor(app: &mut App, mut editor: AgentPresetEditor) {
    let mut presets = crate::agent_presets::load();
    let preset = match editor.validate(&presets) {
        Ok(preset) => preset,
        Err(error) => {
            editor.reject(error);
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
    reopen_presets_list(app, editor.worktree, editor.quick, index);
}

/// `Ctrl+d` in the AGENT PRESETS list: the confirm that guards the delete.
/// The list comes back either way (see `PendingAction::DeleteAgentPreset`).
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
            quick: view.quick.clone().map(Box::new),
        },
        area: ratatui::layout::Rect::default(),
    }));
}

/// The list's Enter: ask for the optional task that the preset's prefix and
/// postfix will wrap — or, for a `skip_task` preset, launch on them at once,
/// through the same submit an empty task box takes. A harness switched off
/// in Settings → Agents is refused here, where the row is, rather than by a
/// failed spawn later.
pub(crate) fn open_agent_preset_task(
    app: &mut App,
    view: &AgentPresetsView,
    out: &mut Vec<ClientRequest>,
) {
    let Some(preset) = view.presets.get(view.selected).cloned() else {
        app.flash = Some("no preset selected — Ctrl+a creates one".into());
        return;
    };
    if !crate::config::Config::load().preset_harness_usable(&preset) {
        app.flash = Some(format!(
            "{} is turned off in Settings → Agents",
            preset
                .custom_harness
                .as_deref()
                .unwrap_or(preset.kind.as_str())
        ));
        return;
    }
    let skip = preset.skip_task;
    let kind = PromptKind::AgentPresetTask {
        worktree: view.worktree.clone(),
        preset,
    };
    if skip {
        crate::event_loop::submit_prompt_now(app, kind, out);
    } else {
        crate::event_loop::open_prompt(app, kind);
    }
}

/// The picker's Enter: adopt the hovered AGENT PRESET — its harness,
/// MODEL / EFFORT and prefix/postfix — for the QUICK PROMPT that opened
/// the list, and hand the box back with its text — or, for a `skip_task`
/// preset picked over an empty box, launch it as Enter on that box would;
/// typed text stays the user's to send. A harness switched off in
/// Settings → Agents is refused here, where the row is, rather than by a
/// failed spawn later.
fn apply_preset_to_quick_prompt(
    app: &mut App,
    presets: &[AgentPreset],
    selected: usize,
    back: crate::quick_prompt::QuickReturn,
    out: &mut Vec<ClientRequest>,
) {
    let Some(preset) = presets.get(selected).cloned() else {
        app.flash = Some("no preset selected".into());
        return;
    };
    let cfg = crate::config::Config::load();
    if !cfg.preset_harness_usable(&preset) {
        app.flash = Some(format!(
            "{} is turned off in Settings → Agents",
            preset
                .custom_harness
                .as_deref()
                .unwrap_or(preset.kind.as_str())
        ));
        return;
    }
    let launch_now = preset.skip_task && back.text.trim().is_empty();
    let launch = crate::quick_prompt::QuickLaunch::of_preset(back.launch.target, preset, &cfg)
        .with_issue(back.launch.issue)
        .with_pr(back.launch.pr);
    if launch_now {
        crate::event_loop::submit_prompt_now(app, PromptKind::QuickPrompt(launch), out);
    } else {
        crate::quick_prompt::reopen(app, launch, &back.text);
    }
}

/// Keys in the AGENT PRESETS list. Letters type ahead over the names — so
/// the verbs are `Ctrl` chords and ↑/↓ (or `Ctrl+n` / `Ctrl+p`) move, as
/// in every filtered overlay.
pub(crate) fn handle_list_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Two-stage, like every type-ahead: the letters first.
        KeyCode::Esc if !view.filter.is_empty() => view.filter.clear(),
        // Backing out of the box's own picker is a return trip, not a
        // close: the box comes back exactly as it left, text and all. A
        // picker reached with no box up (`e` on a pull request or an
        // issue) closes as the manager does — it used to put up an empty
        // box nobody asked for.
        KeyCode::Esc => match view.box_behind().cloned() {
            Some(back) => crate::quick_prompt::reopen(app, back.launch, &back.text),
            None => app.overlay = None,
        },
        KeyCode::Down => view.step(1),
        KeyCode::Up => view.step(-1),
        KeyCode::Char('n') if ctrl => view.step(1),
        KeyCode::Char('p') if ctrl => view.step(-1),
        KeyCode::Char('u') if ctrl => view.filter.clear(),
        // The manage verbs answer in a picker exactly as in the manager:
        // wherever the list shows, its presets can be added, edited and
        // deleted. The editor and the delete confirm carry the picker's
        // box (`view.quick`) and reopen the list as that picker — left to
        // reopen the manager, they dropped the box, its text and the pull
        // request, and the next Enter launched a plain session into the
        // picker's context checkout (for a PR SESSION, the ROOT WORKTREE).
        KeyCode::Char('a') if ctrl => {
            let (worktree, quick) = (view.worktree.clone(), view.quick.clone());
            open_agent_preset_editor(app, worktree, quick, None);
        }
        KeyCode::Char('e') if ctrl => {
            if view.presets.is_empty() {
                app.flash = Some("no preset selected — Ctrl+a creates one".into());
            } else {
                let (worktree, quick) = (view.worktree.clone(), view.quick.clone());
                let index = view.selected;
                open_agent_preset_editor(app, worktree, quick, Some(index));
            }
        }
        // `Delete` too: it types nothing, so it can stay a verb.
        KeyCode::Char('d') if ctrl => {
            let view = view.clone();
            open_delete_preset_confirm(app, &view);
        }
        KeyCode::Delete => {
            let view = view.clone();
            open_delete_preset_confirm(app, &view);
        }
        KeyCode::Enter => activate_selected(app, out),
        // Type-ahead: Backspace widens; a letter narrows the rows to the
        // names it fuzzy matches, the cursor on the best. A letter no name
        // matches is refused, so the list never empties under the user.
        KeyCode::Backspace => view.pop_filter(),
        // A leading space would narrow nothing; it waits for a word.
        KeyCode::Char(' ') if view.filter.is_empty() => {}
        KeyCode::Char(c) if !ctrl => {
            let query = format!("{}{c}", view.filter);
            if !view.type_filter(c) {
                app.flash = Some(if view.presets.is_empty() {
                    "no presets yet — Ctrl+a creates one".into()
                } else {
                    format!("no preset matches '{query}'")
                });
            }
        }
        _ => {}
    }
}

/// The selected row is chosen — Enter, or a click on it. In a QUICK PROMPT
/// picker the row is handed to the box waiting behind the list
/// (`view.quick`), which keeps that box's target, its text, its issue and
/// its pull request; in the manager it launches into the list's worktree,
/// through the preset's task box or past it. The one function both input
/// handlers call: when the click ran a copy of the manager half whatever
/// the mode, a click in the PR SESSION picker — whose worktree is the ROOT
/// WORKTREE, there only to name the project — started a plain session in
/// the main checkout.
fn activate_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::AgentPresets(view)) = &app.overlay else {
        return;
    };
    let view = view.clone();
    match view.quick {
        Some(back) => apply_preset_to_quick_prompt(app, &view.presets, view.selected, back, out),
        None => open_agent_preset_task(app, &view, out),
    }
}

/// Keys in the PRESET EDITOR: Tab order between fields, choice cycling,
/// text editing, save and back.
pub(crate) fn handle_editor_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::AgentPresetEditor(editor)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Esc first clears a choice row's type-ahead, then backs out.
        KeyCode::Esc if !editor.filter.is_empty() => editor.filter.clear(),
        // Back to the list it came from — the manager or the picker —
        // unsaved.
        KeyCode::Esc => {
            let (worktree, quick) = (editor.worktree.clone(), editor.quick.clone());
            let index = editor.editing.unwrap_or(0);
            reopen_presets_list(app, worktree, quick, index);
        }
        // ↓/↑ inside the prefix or postfix walk its lines first, as in
        // Claude Code's prompt; past the last (or first) line — and on
        // every other row — they step fields, as Tab does. Leaving a
        // choice row drops its type-ahead.
        KeyCode::Tab | KeyCode::Down => {
            let walked = key.code == KeyCode::Down
                && editor
                    .text_field_mut()
                    .is_some_and(|input| input.handle_key(&key).consumed());
            if !walked {
                editor.filter.clear();
                editor.field = editor.field.step(editor, 1);
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            let walked = key.code == KeyCode::Up
                && editor
                    .text_field_mut()
                    .is_some_and(|input| input.handle_key(&key).consumed());
            if !walked {
                editor.filter.clear();
                editor.field = editor.field.step(editor, -1);
            }
        }
        // A line break in the prefix or postfix — Shift+Enter, Option+Enter
        // or Ctrl+J, as in the task editor — is the field's own (the `_`
        // arm below); the guard keeps the save off a shifted Enter.
        KeyCode::Enter
            if !editor
                .text_field()
                .is_some_and(|input| input.takes_newline(&key)) =>
        {
            let editor = editor.clone();
            save_agent_preset_editor(app, editor);
        }
        KeyCode::Left if !editor.field.is_typed() => editor.cycle(-1),
        KeyCode::Right | KeyCode::Char(' ') if !editor.field.is_typed() => editor.cycle(1),
        // Letters on a choice row type ahead: the row jumps to the best
        // match and ←/→ cycle the matches. A letter nothing matches is
        // refused, so the row never shows a choice the filter denies.
        KeyCode::Backspace if !editor.field.is_typed() => editor.pop_filter(),
        KeyCode::Char(c) if !editor.field.is_typed() && !ctrl => {
            let query = format!("{}{c}", editor.filter);
            if !editor.type_filter(c) {
                app.flash = Some(format!("no choice matches '{query}'"));
            }
        }
        _ => editor.edit_text(&key),
    }
}

/// Mouse in the AGENT PRESETS list: the wheel moves the selection, a click
/// on a row launches it (rows are actions, as in the hosts picker — editing
/// is `Ctrl+e`), a click outside the modal closes; everything else is swallowed.
/// A click is Enter on that row, in both modes: in a QUICK PROMPT picker it
/// hands the row to the box waiting behind the list (`view.quick`), which
/// keeps that box's target, text, issue and pull request. It used to launch
/// the row into `view.worktree` whatever the mode — for a PR SESSION picker
/// that is the ROOT WORKTREE, which only names the project — so a click on
/// a `skip`-task preset started a plain session in the main checkout, with
/// no pull request and no checkout cut.
pub(crate) fn handle_list_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            view.step(-1);
            app.dirty = true;
        }
        MouseEventKind::ScrollDown => {
            view.step(1);
            app.dirty = true;
        }
        // Rows are the visible ones — whatever the type-ahead left.
        MouseEventKind::Down(MouseButton::Left) => {
            let list = view.list_area;
            let first = view.window_start(list.height as usize);
            let rows = view.visible().len();
            if let Some(index) = crate::list_hit::row_at(list, first, rows, mouse_pos)
                .and_then(|pos| view.visible_row(pos))
            {
                view.selected = index;
                activate_selected(app, out);
            }
            app.dirty = true;
        }
        _ => {}
    }
}

/// The key hint on the list's frame, by mode: where Esc goes is the one
/// thing that differs between the box's picker and one opened on a pull
/// request or an issue — until letters are typed, when Esc clears them.
/// Letters type ahead, so the verbs are `Ctrl` chords (`^a`).
fn modal_hint(view: &AgentPresetsView) -> String {
    format!(" {} ", footer_keys(view))
}

/// The FOOTER's hint while the list is up, by mode, as the frame's.
pub(crate) fn footer_hint(view: &AgentPresetsView) -> String {
    format!("type: filter  ↑/↓: select  {}", footer_keys(view))
}

/// The keys both hints share: Enter's verb, the manage chords, and where
/// Esc goes.
fn footer_keys(view: &AgentPresetsView) -> String {
    let enter = if view.is_picker() { "use" } else { "launch" };
    let esc = if !view.filter.is_empty() {
        "clear"
    } else if view.box_behind().is_some() {
        "back to the prompt"
    } else {
        "close"
    };
    format!("Enter: {enter}  ^a: new  ^e: edit  ^d: delete  Esc: {esc}")
}

/// The AGENT PRESETS list modal.
pub(crate) fn draw_list(f: &mut Frame, app: &mut App, view: &AgentPresetsView, th: Theme) {
    let total = view.presets.len();
    let visible = view.visible();
    let cursor = view.cursor(&visible);
    let selected = visible.get(cursor).map_or(0, |(index, _)| *index);
    // Sized for every preset, not the filtered few, so the frame holds
    // still under the typing.
    let height = (total.max(1) as u16)
        .saturating_add(2)
        .clamp(5, f.area().height.max(5));
    let area = centered_rect(f.area(), AGENT_PRESETS_W, height);
    f.render_widget(Clear, area);
    // In QUICK PROMPT picker mode Enter applies the row to the launch
    // behind it — the box, which Esc gives back, or the pull request or
    // issue the list was opened on, where Esc just closes; the manage
    // verbs are the manager's.
    // The type-ahead shows in the title, as a submenu's does: `Agent
    // presets ⌕ rev`, the bare ⌕ while nothing is typed yet.
    let title = if view.is_picker() {
        "Use preset"
    } else {
        "Agent presets"
    };
    let title = match view.filter.as_str() {
        "" => format!(" {title} ⌕ "),
        query => format!(" {title} ⌕ {query} "),
    };
    let hint = modal_hint(view);
    let block = modal_block(&title, th)
        .title_bottom(Line::from(Span::styled(hint, Style::default().fg(th.dim))));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if total == 0 {
        empty_list_row(f, inner, "no presets yet — Ctrl+a creates one", th);
    }
    let start = window_start(cursor, inner.height as usize);
    for (pos, (i, lit)) in visible.iter().enumerate().skip(start) {
        let preset = &view.presets[*i];
        let Some(row_area) = row_rect(inner, pos - start) else {
            break;
        };
        let budget = (inner.width as usize).saturating_sub(2);
        // "name  claude · opus · high" left, a dim "+prefix +postfix
        // no task" pinned right for what the preset adds to a launch.
        let mut marks = Vec::new();
        if !preset.prefix.trim().is_empty() {
            marks.push("+prefix");
        }
        if !preset.postfix.trim().is_empty() {
            marks.push("+postfix");
        }
        if preset.skip_task {
            marks.push("no task");
        }
        let marks = marks.join(" ");
        let marks_w = marks.chars().count();
        let text_budget = budget.saturating_sub(if marks_w > 0 { marks_w + 2 } else { 0 });
        // The letters the type-ahead matched are lit in the name.
        let name_txt = truncate(&preset.name, text_budget);
        let mut used = name_txt.chars().count();
        let lit = visible_positions(lit, &name_txt, &preset.name);
        let mut spans = fuzzy_highlight_spans(&name_txt, lit, th);
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
        render_row(f, row_area, spans, pos == cursor, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing, plus the clamped cursor.
    if let Some(Overlay::AgentPresets(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = inner;
        v.selected = selected;
    }
}

/// The PRESET EDITOR form — under a banner saying why the last Enter saved
/// nothing, while that still applies, with the field it names in the error
/// color.
pub(crate) fn draw_editor(f: &mut Frame, app: &mut App, editor: &AgentPresetEditor, th: Theme) {
    // Five single rows, a blank, the text boxes, then the Task row — all
    // one row down while the banner is up; the boxes give up rows first on
    // a short screen, down to one line each. A lone side takes both boxes'
    // rows, so the frame never resizes as the Text row cycles.
    let banner = u16::from(editor.error.is_some());
    let frame_h = f.area().height.max(10);
    let box_h = ((frame_h
        .min(PRESET_EDITOR_H + banner)
        .saturating_sub(9 + banner))
        / 2)
    .clamp(3, 6);
    let height = 9 + banner + 2 * box_h;
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

    // The banner: the refusal in the error color on the first row, right
    // above the Name row it points at.
    if let Some(error) = &editor.error {
        if let Some(row_area) = row_rect(inner, 0) {
            let budget = (inner.width as usize).saturating_sub(2);
            let spans = vec![Span::styled(
                truncate(&format!("✗ {}", error.message), budget),
                Style::default().fg(th.err).add_modifier(Modifier::BOLD),
            )];
            render_row(f, row_area, spans, false, true, th);
        }
    }

    let label_w = 9usize;
    let top = usize::from(banner);
    // The five rows above the boxes, and the Task row below them.
    let single = [
        (PresetField::Name, top),
        (PresetField::Kind, top + 1),
        (PresetField::Model, top + 2),
        (PresetField::Effort, top + 3),
        (PresetField::Text, top + 4),
        (PresetField::Task, top + 6 + 2 * box_h as usize),
    ];
    for (field, row) in single.iter() {
        let Some(row_area) = row_rect(inner, *row) else {
            break;
        };
        let focused = editor.field == *field;
        let available = field.available(editor);
        // The field the banner points at: label, value and caret in the
        // error color, focused or not, until its text changes.
        let bad = editor.error.as_ref().is_some_and(|e| e.field == *field);
        let label_style = if bad {
            Style::default().fg(th.err).add_modifier(Modifier::BOLD)
        } else if focused {
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
                    let caret = if bad { th.err } else { th.accent };
                    let mut value = input_spans(&editor.name, budget, caret, th);
                    if bad {
                        for span in &mut value {
                            if span.style.fg == Some(th.text) {
                                span.style.fg = Some(th.err);
                            }
                        }
                    }
                    spans.extend(value);
                } else if editor.name.trim().is_empty() {
                    let hint = if bad { th.err } else { th.dim };
                    spans.push(Span::styled("(required)", Style::default().fg(hint)));
                } else if bad {
                    spans.push(Span::styled(
                        truncate(editor.name.as_str(), budget),
                        Style::default().fg(th.err),
                    ));
                } else {
                    spans.push(Span::raw(truncate(editor.name.as_str(), budget)));
                }
            }
            _ => {
                let value = match field {
                    PresetField::Kind => editor
                        .custom
                        .as_deref()
                        .unwrap_or_else(|| editor.kind.as_str())
                        .to_string(),
                    PresetField::Model => editor.model.clone(),
                    PresetField::Effort => editor.effort.clone(),
                    PresetField::Text => editor.text.as_str().to_string(),
                    PresetField::Task => editor.task_choice().to_string(),
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
                if *field == PresetField::Text {
                    let what = match editor.text {
                        PresetText::Prefix => "  one box, sent before your task",
                        PresetText::Postfix => "  one box, sent after your task",
                        PresetText::Both => "  two boxes, around your task",
                    };
                    spans.push(Span::styled(what, Style::default().fg(th.dim)));
                }
                if *field == PresetField::Task {
                    let what = if editor.skip_task {
                        "  Enter launches at once on prefix + postfix"
                    } else {
                        "  Enter asks for a task (optional)"
                    };
                    spans.push(Span::styled(what, Style::default().fg(th.dim)));
                }
            }
        }
        render_row(f, row_area, spans, focused, true, th);
    }

    // The text boxes under a blank row: one per side the Text row names,
    // stacked when it names both; a lone side takes both boxes' rows.
    let sides = [
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
    let shown: Vec<_> = sides
        .iter()
        .filter(|(field, ..)| field.available(editor))
        .collect();
    let each = if shown.len() > 1 { box_h } else { 2 * box_h };
    let mut focused_view = None;
    for (n, (field, input, title, placeholder)) in shown.into_iter().enumerate() {
        let y = inner.y.saturating_add(banner + 6 + n as u16 * each);
        if y + each > inner.y + inner.height {
            break;
        }
        let box_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: each,
        };
        let focused = editor.field == *field;
        let bad = editor.error.as_ref().is_some_and(|e| e.field == *field);
        let border = if bad {
            th.err
        } else if focused {
            th.accent
        } else {
            th.dim
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .title(Span::styled(*title, Style::default().fg(border)));
        let box_inner = block.inner(box_area);
        f.render_widget(block, box_area);
        if focused {
            let (view, rows) = draw_multiline_input(f, input, box_inner, th);
            draw_scroll_marks(f, box_area, view, rows, th.dim);
            focused_view = Some(view);
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
    // of backs out from, and the view the focused box's rows walk by.
    if let Some(Overlay::AgentPresetEditor(e)) = &mut app.overlay {
        e.area = area;
        if let (Some(view), Some(input)) = (focused_view, e.text_field_mut()) {
            input.set_view(view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::WorktreeId;

    fn pinned<T>(json: &str, f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        crate::config::with_config_path(path, f)
    }

    fn editor(text: PresetText) -> AgentPresetEditor {
        AgentPresetEditor::new(WorktreeId("w1".into()), text)
    }

    fn stored(prefix: &str, postfix: &str) -> AgentPreset {
        AgentPreset {
            name: "reviewer".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: prefix.into(),
            postfix: postfix.into(),
            skip_task: false,
        }
    }

    /// The Text row: one box on the side it names, both when it says so.
    /// Tab order skips the box that isn't there, and a side left out saves
    /// blank — though its text waits in the form until then, so cycling
    /// back brings it back.
    #[test]
    fn the_text_row_picks_the_boxes_and_a_hidden_side_saves_blank() {
        let mut e = editor(PresetText::Prefix);
        assert_eq!(e.text, PresetText::Prefix);
        assert!(PresetField::Prefix.available(&e));
        assert!(!PresetField::Postfix.available(&e));
        assert_eq!(PresetField::Effort.step(&e, 1), PresetField::Text);
        assert_eq!(PresetField::Text.step(&e, 1), PresetField::Prefix);
        assert_eq!(
            PresetField::Prefix.step(&e, 1),
            PresetField::Task,
            "no postfix box to land on"
        );
        assert_eq!(PresetField::Task.step(&e, -1), PresetField::Prefix);

        e.name = TextInput::with_text("p");
        e.prefix = TextInput::with_text("PRE");
        e.postfix = TextInput::with_text("POST");
        let saved = e.to_preset();
        assert_eq!(saved.prefix, "PRE");
        assert_eq!(
            saved.postfix, "",
            "a side the form doesn't show saves blank"
        );

        // ← / → on the Text row walk the sides; the postfix text was
        // waiting all along.
        e.field = PresetField::Text;
        e.cycle(1);
        assert_eq!(e.text, PresetText::Postfix);
        assert_eq!(e.row_value(), "postfix");
        assert!(!PresetField::Prefix.available(&e));
        assert!(PresetField::Postfix.available(&e));
        assert_eq!(PresetField::Text.step(&e, 1), PresetField::Postfix);
        assert_eq!(PresetField::Postfix.step(&e, -1), PresetField::Text);
        let saved = e.to_preset();
        assert_eq!(
            (saved.prefix.as_str(), saved.postfix.as_str()),
            ("", "POST")
        );
        e.cycle(1);
        assert_eq!(e.text, PresetText::Both);
        assert_eq!(e.row_value(), "prefix & postfix");
        assert_eq!(PresetField::Prefix.step(&e, 1), PresetField::Postfix);
        let saved = e.to_preset();
        assert_eq!(
            (saved.prefix.as_str(), saved.postfix.as_str()),
            ("PRE", "POST")
        );
        e.cycle(1);
        assert_eq!(e.text, PresetText::Prefix, "wraps");
        e.cycle(-1);
        assert_eq!(e.text, PresetText::Both, "← steps back");
        assert_eq!(
            e.row_choices(),
            vec!["prefix", "postfix", "prefix & postfix"],
            "the row's choices are the sides, in cycle order"
        );
        // A choice the row doesn't list leaves it alone.
        e.set_row_value("suffix");
        assert_eq!(e.text, PresetText::Both);
    }

    /// Editing a stored preset shows every side that holds text on top of
    /// the setting's — a both-sided preset from before the setting opens
    /// with both boxes under the `prefix` default, and a blank one opens
    /// like a new one.
    #[test]
    fn editing_shows_a_stored_side_whatever_the_setting_says() {
        let w = || WorktreeId("w1".into());
        let post = stored("", "Run the tests.");
        let e = AgentPresetEditor::from_preset(w(), 0, &post, PresetText::Prefix);
        assert_eq!(
            e.text,
            PresetText::Both,
            "the postfix it holds plus the prefix box the setting wants"
        );
        assert_eq!(
            e.to_preset().postfix,
            "Run the tests.",
            "opening the form drops nothing saved"
        );
        let e = AgentPresetEditor::from_preset(w(), 0, &post, PresetText::Postfix);
        assert_eq!(e.text, PresetText::Postfix);
        let e = AgentPresetEditor::from_preset(
            w(),
            0,
            &stored("Be strict.", "Run the tests."),
            PresetText::Prefix,
        );
        assert_eq!(e.text, PresetText::Both);
        let e = AgentPresetEditor::from_preset(w(), 0, &stored("", ""), PresetText::Postfix);
        assert_eq!(
            e.text,
            PresetText::Postfix,
            "a blank preset follows the setting"
        );
    }

    /// The Harness row lists custom entries by id after the built-ins —
    /// never a bare `custom` — and picking one stores the registry id.
    #[test]
    fn kind_choices_list_custom_entries_by_id() {
        pinned(
            r#"{"custom_harnesses": [{"id": "agy", "program": "agy"}]}"#,
            || {
                let mut editor = editor(PresetText::Prefix);
                editor.field = PresetField::Kind;
                let choices = editor.row_choices();
                assert!(choices.contains(&"claude".to_string()), "{choices:?}");
                assert!(choices.contains(&"agy".to_string()), "{choices:?}");
                assert!(!choices.contains(&"custom".to_string()), "{choices:?}");

                editor.set_row_value("agy");
                assert_eq!(editor.kind, AgentKind::Custom);
                assert_eq!(editor.custom.as_deref(), Some("agy"));
                assert_eq!(editor.row_value(), "agy");
                let preset = editor.to_preset();
                assert_eq!(preset.custom_harness.as_deref(), Some("agy"));

                editor.set_row_value("codex");
                assert_eq!(editor.kind, AgentKind::Codex);
                assert_eq!(editor.custom, None);
            },
        );
    }

    /// A refused save moves the caret onto the field the error names,
    /// wherever it was; the first change to that field's text takes the
    /// error down, a change elsewhere leaves it — the name is still the
    /// problem. Pastes land like typed keys: lines kept in the prefix,
    /// flattened in the name, nowhere on a choice row.
    #[test]
    fn reject_points_at_the_field_and_its_first_edit_clears() {
        let mut editor = AgentPresetEditor::new(WorktreeId("w1".into()), PresetText::Prefix);
        let error = editor.validate(&[]).unwrap_err();
        assert_eq!(
            error,
            FormError {
                field: PresetField::Name,
                message: "the preset needs a name".into(),
            }
        );

        editor.field = PresetField::Postfix;
        editor.filter = "x".into();
        editor.reject(error.clone());
        assert_eq!(
            editor.field,
            PresetField::Name,
            "the caret jumps to the fix"
        );
        assert_eq!(editor.filter, "", "and no type-ahead comes along");
        assert_eq!(editor.error.as_ref(), Some(&error));

        // Typing into another field leaves the banner up.
        editor.field = PresetField::Prefix;
        assert!(editor.paste("one\ntwo"));
        assert_eq!(editor.prefix.as_str(), "one\ntwo", "lines kept in the box");
        assert_eq!(editor.error.as_ref(), Some(&error));

        // A choice row has nowhere to paste, and changes nothing.
        editor.field = PresetField::Kind;
        assert!(!editor.paste("codex"));
        assert_eq!(editor.row_value(), "claude");
        assert_eq!(editor.error.as_ref(), Some(&error));

        // The first change to the name is the fix.
        editor.field = PresetField::Name;
        assert!(editor.paste("re\nviewer"));
        assert_eq!(
            editor.name.as_str(),
            "re viewer",
            "flattened in the one-line field"
        );
        assert_eq!(editor.error, None);
        assert!(editor.validate(&[]).is_ok());

        // Likewise a typed key — but only one that changes the text: an
        // arrow leaves the banner where it is.
        editor.reject(error.clone());
        editor.edit_text(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(editor.error.as_ref(), Some(&error));
        editor.edit_text(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.error, None);
    }
}
