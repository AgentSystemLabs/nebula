//! The SKILL PICKER: `Ctrl+S` in the QUICK PROMPT box (or `/` typed into
//! an empty one) and in the FOLLOW-UP box — tick the Claude skills this
//! one prompt should use, and the box comes back with its text intact and
//! the pick on it ([`crate::skills::SkillPick`]).
//!
//! The list is a [`FilterList`]: the project's SKILL WORKFLOWS first —
//! named skill sets from its `.orbit/config.json`, where choosing one ticks
//! (or unticks) its whole set — then every skill, labelled by where it was
//! found. `Space` (or a click) ticks the row under the cursor, `Enter`
//! confirms, `Esc` clears the filter and then cancels, and `Ctrl+W` saves
//! what is ticked as a workflow in the project's main checkout.
//!
//! Like every picker opened from the box it owes the box back
//! ([`SkillsFor`]): the QUICK PROMPT's launch and text ride a
//! [`QuickReturn`], so neither the launch spec nor the typed text is lost,
//! and the FOLLOW-UP box's session, text and pick ride along the same way.
//!
//! The rows come from disk — a hundred small files — so opening never
//! reads them: it shows the project's last catalog (`JobState::
//! skill_catalogs`) at once and asks a BACKGROUND READ for a fresh one
//! (`view_jobs::Answer::Skills`), which lands in [`land_catalog`].

use crate::app::{App, Overlay, PromptKind};
use crate::filter_list::FilterList;
use crate::quick_prompt::{QuickReturn, QuickTarget};
use crate::skills::{Catalog, Skill, SkillPick, Workflow};
use crate::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orbit_core::AgentId;
use ratatui::layout::{Position, Rect};
use std::path::PathBuf;

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickRow {
    Workflow(Workflow),
    Skill(Skill),
}

impl PickRow {
    /// What the filter matches: the name alone — descriptions are long
    /// enough that a fuzzy match on them hits nearly every row.
    pub fn label(&self) -> &str {
        match self {
            Self::Workflow(w) => &w.name,
            Self::Skill(s) => &s.name,
        }
    }
}

/// The box the picker hands back.
#[derive(Debug, Clone, PartialEq)]
pub enum SkillsFor {
    /// A QUICK PROMPT: its launch (the pick goes on `launch.skills`) and
    /// the text typed so far.
    Quick(Box<QuickReturn>),
    /// The FOLLOW-UP box for session `id`: what was typed, and the pick
    /// the box held.
    FollowUp {
        id: AgentId,
        text: String,
        pick: SkillPick,
    },
}

impl SkillsFor {
    fn pick(&self) -> &SkillPick {
        match self {
            Self::Quick(back) => &back.launch.skills,
            Self::FollowUp { pick, .. } => pick,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillPickerView {
    pub list: FilterList<PickRow>,
    /// The ticked skills, in the order they were ticked.
    pub chosen: Vec<String>,
    pub back: SkillsFor,
    /// The checkout the box is aimed at, and the project's main checkout
    /// (where workflows are saved, and the repo root plugins are matched
    /// against). The catalog cache is keyed by the first.
    pub checkout: PathBuf,
    pub main: PathBuf,
    /// Has a catalog landed for this checkout (cached or fresh)?
    pub loaded: bool,
    /// The project file could not be read: its workflows are missing.
    pub error: Option<String>,
    /// `Ctrl+W`'s one-line name prompt, while it is up.
    pub naming: Option<TextInput>,
    /// Opened by typing `/` into an empty QUICK PROMPT: cancelled with
    /// nothing ticked, the `/` (and anything typed after it) goes into
    /// the box after all, so slash commands still type as they always did.
    pub from_slash: bool,
    /// Rects written during draw: the whole modal, for a click outside.
    pub area: Rect,
}

impl SkillPickerView {
    fn new(back: SkillsFor, checkout: PathBuf, main: PathBuf, from_slash: bool) -> Self {
        Self {
            chosen: back.pick().skills.clone(),
            list: FilterList::default(),
            back,
            checkout,
            main,
            loaded: false,
            error: None,
            naming: None,
            from_slash,
            area: Rect::default(),
        }
    }

    /// Swap in `catalog`'s rows, keeping the ticks, the query and (where
    /// it is still listed) the row under the cursor.
    pub fn set_catalog(&mut self, catalog: Catalog) {
        let at = self.list.selected().map(|row| row.label().to_string());
        let mut rows: Vec<PickRow> = catalog
            .workflows
            .into_iter()
            .map(PickRow::Workflow)
            .collect();
        rows.extend(catalog.skills.into_iter().map(PickRow::Skill));
        self.list.items = rows;
        self.error = catalog.error;
        self.loaded = true;
        self.apply_filter();
        if let Some(label) = at {
            if let Some(item) = self.list.items.iter().position(|r| r.label() == label) {
                self.list.move_to_item(item);
            }
        }
    }

    pub fn apply_filter(&mut self) {
        self.list.apply_filter(|row| row.label().to_string());
    }

    pub fn is_chosen(&self, skill: &str) -> bool {
        self.chosen.iter().any(|s| s == skill)
    }

    /// Is every skill of `workflow` ticked? A workflow with no skills
    /// never is.
    pub fn workflow_on(&self, workflow: &Workflow) -> bool {
        !workflow.skills.is_empty() && workflow.skills.iter().all(|s| self.is_chosen(s))
    }

    /// Tick or untick the row under the cursor: a skill alone, or a
    /// workflow's whole set — off when all of it is on, on otherwise.
    /// What `Space` and a click both do.
    pub fn toggle_selected(&mut self) {
        match self.list.selected().cloned() {
            Some(PickRow::Skill(skill)) => {
                if self.is_chosen(&skill.name) {
                    self.chosen.retain(|s| s != &skill.name);
                } else {
                    self.chosen.push(skill.name);
                }
            }
            Some(PickRow::Workflow(workflow)) => {
                if self.workflow_on(&workflow) {
                    self.chosen.retain(|s| !workflow.skills.contains(s));
                } else {
                    for skill in workflow.skills {
                        if !self.is_chosen(&skill) {
                            self.chosen.push(skill);
                        }
                    }
                }
            }
            None => {}
        }
    }

    /// What is ticked, as the box will carry it: the skills, and the
    /// extra text of every listed workflow whose whole set is ticked.
    pub fn pick(&self) -> SkillPick {
        let mut texts: Vec<&str> = Vec::new();
        for row in &self.list.items {
            if let PickRow::Workflow(w) = row {
                if let Some(text) = w.text.as_deref().filter(|_| self.workflow_on(w)) {
                    if !texts.contains(&text) {
                        texts.push(text);
                    }
                }
            }
        }
        // A pick the box already held keeps its text while its workflows
        // are not listed yet (the catalog has not landed).
        let text = if texts.is_empty() && !self.loaded && self.chosen == self.back.pick().skills {
            self.back.pick().text.clone()
        } else {
            (!texts.is_empty()).then(|| texts.join("\n\n"))
        };
        SkillPick {
            skills: self.chosen.clone(),
            text: text.filter(|_| !self.chosen.is_empty()),
        }
    }
}

/// The checkout a box is aimed at and its project's main checkout, or
/// None when either is gone.
fn places(app: &App, back: &SkillsFor) -> Option<(PathBuf, PathBuf)> {
    let worktree = match back {
        SkillsFor::Quick(back) => match &back.launch.target {
            QuickTarget::Worktree(id) => app.tree.worktrees.iter().find(|w| &w.id == id)?,
            QuickTarget::NewWorktree { project, .. } => {
                let main = app
                    .tree
                    .projects
                    .iter()
                    .find(|p| &p.id == project)?
                    .repo_path
                    .clone();
                return Some((main.clone(), main));
            }
        },
        SkillsFor::FollowUp { id, .. } => {
            let agent = app.tree.agents.iter().find(|a| &a.id == id)?;
            app.tree
                .worktrees
                .iter()
                .find(|w| w.id == agent.worktree_id)?
        }
    };
    let main = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == worktree.project_id)?
        .repo_path
        .clone();
    Some((worktree.path.clone(), main))
}

/// The QUICK PROMPT or FOLLOW-UP box `prompt` is, as the picker carries
/// it back; None for every other prompt.
pub(crate) fn skills_for(prompt: &crate::app::PromptDialog) -> Option<SkillsFor> {
    match &prompt.kind {
        PromptKind::QuickPrompt(launch) => Some(SkillsFor::Quick(Box::new(QuickReturn {
            launch: launch.clone(),
            text: prompt.input.as_str().to_string(),
            from_box: true,
        }))),
        PromptKind::FollowUp { id, skills } => Some(SkillsFor::FollowUp {
            id: id.clone(),
            text: prompt.input.as_str().to_string(),
            pick: skills.clone(),
        }),
        _ => None,
    }
}

/// Put the picker up over the box `back` came from, on the project's last
/// catalog when there is one, and ask for a fresh one.
pub(crate) fn open(app: &mut App, back: SkillsFor, from_slash: bool) {
    let Some((checkout, main)) = places(app, &back) else {
        app.chrome.flash = Some("skills: the box's project no longer exists".into());
        return;
    };
    let mut view = SkillPickerView::new(back, checkout.clone(), main.clone(), from_slash);
    if let Some(catalog) = app.jobs.skill_catalogs.get(&checkout).cloned() {
        view.set_catalog(catalog);
    }
    app.modals.overlay = Some(Overlay::SkillPicker(Box::new(view)));
    app.chrome.dirty = true;
    request_catalog(app, checkout, main);
}

/// Read the catalog for `checkout` off the loop and land it — inline when
/// there is no loop (unit tests).
fn request_catalog(app: &mut App, checkout: PathBuf, main: PathBuf) {
    let home = crate::skills::claude_home();
    match app.jobs.view_jobs.clone() {
        Some(jobs) => jobs.run(move || {
            let catalog = crate::skills::load_catalog(&home, &checkout, &main);
            Some(crate::view_jobs::Answer::Skills { checkout, catalog })
        }),
        None => {
            let catalog = crate::skills::load_catalog(&home, &checkout, &main);
            land_catalog(app, checkout, catalog);
        }
    }
}

/// A catalog read landing: cached for the next open, and swapped into the
/// picker when it is up for that checkout.
pub(crate) fn land_catalog(app: &mut App, checkout: PathBuf, catalog: Catalog) {
    if let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay {
        if view.checkout == checkout {
            view.set_catalog(catalog.clone());
            app.chrome.dirty = true;
        }
    }
    app.jobs.skill_catalogs.insert(checkout, catalog);
}

/// Hand the box back: with `pick` on it when confirmed (None keeps the
/// one it had), and `typed` added to its text.
fn hand_back(app: &mut App, back: SkillsFor, pick: Option<SkillPick>, typed: &str) {
    app.modals.overlay = None;
    match back {
        SkillsFor::Quick(back) => {
            let mut launch = back.launch;
            if let Some(pick) = pick {
                launch.skills = pick;
            }
            let text = format!("{}{typed}", back.text);
            crate::quick_prompt::reopen(app, launch, &text);
        }
        SkillsFor::FollowUp {
            id,
            text,
            pick: had,
        } => {
            let text = format!("{text}{typed}");
            crate::event_loop::open_follow_up_with(app, id, pick.unwrap_or(had), text);
        }
    }
}

/// `Enter`: back to the box with what is ticked. Nothing ticked but a
/// query typed takes the row the query found — type, Enter, as in any
/// fuzzy picker; nothing ticked and nothing typed clears the box's pick.
pub(crate) fn confirm(app: &mut App) {
    let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay else {
        return;
    };
    if view.chosen.is_empty() && view.list.query_has_words() {
        view.toggle_selected();
    }
    let pick = view.pick();
    let back = view.back.clone();
    hand_back(app, back, Some(pick), "");
}

/// The second `Esc` (or a click outside): back to the box as it was. A
/// picker the `/` key opened with nothing ticked types that `/` — and
/// whatever query followed it — into the box after all.
pub(crate) fn cancel(app: &mut App) {
    let Some(Overlay::SkillPicker(view)) = &app.modals.overlay else {
        return;
    };
    let typed = if view.from_slash && view.chosen.is_empty() {
        format!("/{}", view.list.query.as_str())
    } else {
        String::new()
    };
    let back = view.back.clone();
    hand_back(app, back, None, &typed);
}

/// Tick or untick the row at match index `row` — the one action behind
/// `Space` (on the cursor's row) and a click (on the row under the
/// pointer). INPUT PARITY.
pub(crate) fn toggle_row(app: &mut App, row: usize) {
    if let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay {
        view.list.select(row as i64);
        view.toggle_selected();
        app.chrome.dirty = true;
    }
}

/// `Ctrl+W`'s name, entered: save what is ticked as a workflow in the
/// main checkout's `.orbit/config.json`, list it, and say where it went.
fn save_workflow(app: &mut App) {
    let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay else {
        return;
    };
    let name = view
        .naming
        .as_ref()
        .map(|n| n.as_str().trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        app.chrome.flash = Some("a workflow needs a name".into());
        return;
    }
    let pick = view.pick();
    let workflow = orbit_core::project_file::Workflow {
        skills: pick.skills.clone(),
        text: pick.text.clone(),
    };
    // One small file, written tmp-then-rename like a settings save.
    match orbit_core::project_file::write_workflow(&view.main, &name, &workflow) {
        Ok(path) => {
            view.naming = None;
            let row = PickRow::Workflow(Workflow {
                name: name.clone(),
                skills: workflow.skills,
                text: workflow.text,
            });
            match view
                .list
                .items
                .iter()
                .position(|r| matches!(r, PickRow::Workflow(w) if w.name == name))
            {
                Some(at) => view.list.items[at] = row,
                None => {
                    let at = view
                        .list
                        .items
                        .iter()
                        .position(|r| matches!(r, PickRow::Skill(_)))
                        .unwrap_or(view.list.items.len());
                    view.list.items.insert(at, row);
                }
            }
            view.apply_filter();
            // The cached catalog is stale now; the next open re-reads.
            let checkout = view.checkout.clone();
            app.jobs.skill_catalogs.remove(&checkout);
            app.chrome.flash = Some(format!("saved workflow '{name}' to {}", path.display()));
        }
        Err(e) => app.chrome.flash = Some(format!("could not save the workflow: {e}")),
    }
}

/// Keys while the picker is up.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay else {
        return;
    };
    app.chrome.dirty = true;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if let Some(naming) = &mut view.naming {
        match key.code {
            KeyCode::Esc => view.naming = None,
            KeyCode::Enter => save_workflow(app),
            _ => {
                naming.handle_key(&key);
            }
        }
        return;
    }
    match key.code {
        KeyCode::Enter => confirm(app),
        KeyCode::Char(' ') if !ctrl => {
            let row = view.list.cursor;
            toggle_row(app, row);
        }
        KeyCode::Char('w' | 'W') if ctrl => {
            if view.chosen.is_empty() {
                app.chrome.flash = Some("tick the skills the workflow should use first".into());
            } else {
                view.naming = Some(TextInput::new());
            }
        }
        // Esc with a query clears it — unless the `/` key opened this and
        // nothing is ticked: then it is a slash command being typed, and
        // it goes into the box whole.
        KeyCode::Esc
            if !view.list.query.is_empty() && !(view.from_slash && view.chosen.is_empty()) =>
        {
            view.list.query.clear();
            view.apply_filter();
        }
        KeyCode::Esc => cancel(app),
        _ => {
            let page = view.list.list_area.height.max(1) as i64;
            if let crate::filter_list::FilterListKey::QueryChanged =
                view.list.handle_standard_key(&key, page)
            {
                view.apply_filter();
            }
        }
    }
}

/// The pointer while the picker is up: the wheel moves the cursor, a click
/// on a row ticks it ([`toggle_row`], the one `Space` runs).
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            view.list.step(-1);
        }
        MouseEventKind::ScrollDown => {
            view.list.step(1);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if view.naming.is_none() {
                if let Some(row) = view.list.hit(pos) {
                    toggle_row(app, row);
                }
            }
        }
        _ => return,
    }
    app.chrome.dirty = true;
}

/// A paste lands in the name prompt while it is up, else in the query.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::SkillPicker(view)) = &mut app.modals.overlay else {
        return false;
    };
    let line = text.replace(['\n', '\r'], " ");
    match &mut view.naming {
        Some(naming) => naming.insert_str(&line),
        None => {
            view.list.query.insert_str(&line);
            view.apply_filter();
        }
    }
    true
}

/// The footer bar's keys while the picker is up.
pub(crate) fn footer_hint(view: &SkillPickerView) -> &'static str {
    if view.naming.is_some() {
        "type the workflow's name  Enter: save to .orbit/config.json  Esc: back to the list"
    } else {
        "type: filter  Space: tick  Enter: done  Ctrl+W: save as workflow  ↑/↓: move  Esc: clear/back to the box"
    }
}
