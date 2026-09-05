//! FILE TABS — the modal `nebula open <file>…` raises from inside a session:
//! a tab strip (the SETTINGS OVERLAY's) with one tab per file, the focused
//! tab's file previewed underneath (the TREE BROWSER's highlighted preview),
//! and Enter turning that pane into the EDITOR. It is the one modal where
//! the HARDWIRED UNLOCK backs out a level instead of closing outright: out
//! of the editor or the preview, Ctrl+Q steps back to the strip, and only
//! from the strip does it close — asked for on 2026-09-04, so a file can be
//! read, edited and left without ever losing the set of tabs by accident.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::{App, Overlay};
use crate::syntax::{Highlighter, TokenKind};
use crate::tree_browser::read_preview;

/// Widest a tab label gets; a handful have to fit side by side, and the
/// tail of a path is the part that tells files apart.
const MAX_LABEL_CHARS: usize = 28;
/// Preview lines a wheel notch moves — the PR PREVIEW's pace, this being a
/// document to read rather than a terminal to follow.
const WHEEL_LINES: i32 = 3;

/// One tab: the file (absolute) and the label the strip shows for it.
#[derive(Debug, Clone)]
pub struct FileTab {
    pub path: PathBuf,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct FileTabsView {
    /// The agent's checkout: the editor's cwd, and what labels are relative to.
    pub root: PathBuf,
    /// Editor command Enter launches (NEBULA_EDITOR, then the `editor`
    /// setting, default vim), captured at open time.
    pub editor: String,
    pub tabs: Vec<FileTab>,
    /// Index into `tabs`.
    pub tab: usize,
    /// The strip has the cursor: ←/→ walk the tabs, ↓ drops into the
    /// preview. False while the preview scrolls under j/k; Esc and Ctrl+Q
    /// from there land back here before anything closes.
    pub on_tabs: bool,
    /// The focused tab's file as highlighted (kind, text) runs per line;
    /// styling lives in ui.rs.
    pub preview_lines: Vec<Vec<(TokenKind, String)>>,
    pub preview_line_count: usize,
    /// Real file contents — the case that earns a line-number gutter —
    /// rather than a read error or placeholder.
    pub preview_is_file: bool,
    /// Top visible preview line.
    pub scroll: u16,
    /// Inner height of the preview, written back during draw so paging
    /// tracks resizes.
    pub view_height: u16,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen x-range of each tab label, written during draw so clicks on
    /// the strip land on the right tab.
    pub tab_hits: Vec<(u16, u16)>,
    /// The preview pane's inner rect, written back during draw; the
    /// embedded editor spawns and renders at this size.
    pub body_area: Rect,
}

impl FileTabsView {
    /// Tabs on `paths` in the order given, the first one previewed.
    pub fn new(root: PathBuf, editor: String, paths: Vec<PathBuf>) -> Self {
        let tabs = paths
            .into_iter()
            .map(|path| FileTab {
                label: label_for(&root, &path),
                path,
            })
            .collect();
        let mut view = Self {
            root,
            editor,
            tabs,
            tab: 0,
            on_tabs: true,
            preview_lines: Vec::new(),
            preview_line_count: 0,
            preview_is_file: false,
            scroll: 0,
            view_height: 0,
            area: Rect::default(),
            tab_hits: Vec::new(),
            body_area: Rect::default(),
        };
        view.load_preview();
        view
    }

    pub fn selected(&self) -> Option<&FileTab> {
        self.tabs.get(self.tab)
    }

    /// Move to tab `index`, wrapping at both ends, and reload the preview
    /// when it moved.
    pub fn select_tab(&mut self, index: i64) {
        if self.tabs.is_empty() {
            return;
        }
        let next = index.rem_euclid(self.tabs.len() as i64) as usize;
        if next != self.tab {
            self.tab = next;
            self.load_preview();
        }
    }

    /// Re-read the focused tab's file into the preview, scrolled to the top.
    pub fn load_preview(&mut self) {
        self.scroll = 0;
        let (text, is_file) = match self.selected() {
            Some(tab) => match read_preview(&tab.path) {
                Ok(text) => (text, true),
                Err(message) => (message, false),
            },
            None => ("(no files)".to_string(), false),
        };
        let mut hl = match self.selected() {
            Some(tab) if is_file => Highlighter::for_path(&tab.path.to_string_lossy()),
            _ => Highlighter::plain(),
        };
        self.preview_is_file = is_file;
        self.preview_lines = text.lines().map(|l| hl.line(l)).collect();
        self.preview_line_count = self.preview_lines.len();
    }

    pub fn max_scroll(&self) -> u16 {
        self.preview_line_count
            .saturating_sub(self.view_height as usize)
            .min(u16::MAX as usize) as u16
    }

    /// Clamped relative preview scroll.
    pub fn scroll_by(&mut self, delta: i32) {
        let max = self.max_scroll() as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max) as u16;
    }

    /// The editor over this modal went away (Ctrl+Q or the editor's own
    /// quit): show the file as it is now, cursor back on the strip.
    pub fn editor_closed(&mut self) {
        self.load_preview();
        self.on_tabs = true;
    }
}

/// The strip's name for a file: its path under the checkout when it is
/// there, else just the file name — cut from the front when long, since
/// the tail is what tells `a/README.md` from `b/README.md`.
fn label_for(root: &Path, path: &Path) -> String {
    let shown = match path.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
    };
    let count = shown.chars().count();
    if count <= MAX_LABEL_CHARS {
        return shown;
    }
    let tail: String = shown.chars().skip(count - (MAX_LABEL_CHARS - 1)).collect();
    format!("…{tail}")
}

/// `FilesOpened` from the daemon — `nebula open` in a session: raise the
/// modal on the files, over whatever overlay was up. A running editor is
/// left alone (it may hold an unsaved buffer) and keeps drawing as the
/// floating modal above the new tabs; when it quits, the tabs are what is
/// underneath, with the first file freshly read.
pub(crate) fn open(app: &mut App, root: PathBuf, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if let Some(vim) = &mut app.vim {
        vim.embedded = false;
    }
    let editor = crate::config::Config::load().editor_command();
    app.overlay = Some(Overlay::FileTabs(FileTabsView::new(root, editor, paths)));
    app.dirty = true;
}

/// What a key means, decided from where the cursor is (the SETTINGS
/// OVERLAY's pattern): Tab / ⇧Tab / ←/→ / h/l / 1-9 switch tabs from
/// anywhere; ↓/j from the strip drops into the preview, where j/k scroll
/// and ↑ off the top steps back onto the strip; Enter opens the editor;
/// Esc backs out one level, so from the strip it closes.
enum Cmd {
    Close,
    ToStrip,
    IntoPreview,
    Tab(i64),
    Edit,
    Scroll(i32),
    Top,
    Bottom,
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::FileTabs(view)) = &app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let (on_tabs, tab, tabs) = (view.on_tabs, view.tab as i64, view.tabs.len());
    let half = (view.view_height / 2).max(1) as i32;
    let page = view.view_height.max(1) as i32;
    let at_top = view.scroll == 0;

    let cmd = match key.code {
        KeyCode::Esc | KeyCode::Char('q') if on_tabs => Cmd::Close,
        KeyCode::Esc | KeyCode::Char('q') => Cmd::ToStrip,
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => Cmd::Tab(tab - 1),
        KeyCode::Tab if shift => Cmd::Tab(tab - 1),
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => Cmd::Tab(tab + 1),
        // 1-9 jump straight to a tab; out-of-range digits are ignored.
        KeyCode::Char(c @ '1'..='9') => {
            let want = c as i64 - '1' as i64;
            if (want as usize) < tabs {
                Cmd::Tab(want)
            } else {
                return;
            }
        }
        KeyCode::Enter => Cmd::Edit,
        // ---- the strip has the cursor ----
        KeyCode::Down | KeyCode::Char('j') if on_tabs => Cmd::IntoPreview,
        KeyCode::Up | KeyCode::Char('k') if on_tabs => return,
        // ---- the preview has it ----
        KeyCode::Down | KeyCode::Char('j') => Cmd::Scroll(1),
        KeyCode::Up | KeyCode::Char('k') if at_top => Cmd::ToStrip,
        KeyCode::Up | KeyCode::Char('k') => Cmd::Scroll(-1),
        KeyCode::Char('d') if ctrl => Cmd::Scroll(half),
        KeyCode::Char('u') if ctrl => Cmd::Scroll(-half),
        KeyCode::PageDown => Cmd::Scroll(page),
        KeyCode::PageUp => Cmd::Scroll(-page),
        KeyCode::Home | KeyCode::Char('g') => Cmd::Top,
        KeyCode::End | KeyCode::Char('G') => Cmd::Bottom,
        _ => return,
    };

    match cmd {
        Cmd::Close => app.overlay = None,
        Cmd::Edit => open_in_editor(app),
        other => {
            let Some(Overlay::FileTabs(view)) = &mut app.overlay else {
                return;
            };
            match other {
                Cmd::ToStrip => view.on_tabs = true,
                Cmd::IntoPreview => view.on_tabs = false,
                Cmd::Tab(i) => view.select_tab(i),
                Cmd::Scroll(delta) => view.scroll_by(delta),
                Cmd::Top => view.scroll = 0,
                Cmd::Bottom => view.scroll = view.max_scroll(),
                Cmd::Close | Cmd::Edit => unreachable!("handled above"),
            }
        }
    }
    app.dirty = true;
}

/// Mouse inside the modal (a click outside already closed it): a tab label
/// switches to that tab and parks the cursor on the strip, a click in the
/// body puts it in the preview, and the wheel scrolls the preview.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::FileTabs(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // The strip is the first inner row, right under the top border.
            if mouse.row == view.area.y.saturating_add(1) {
                if let Some(i) = view
                    .tab_hits
                    .iter()
                    .position(|(x0, x1)| mouse.column >= *x0 && mouse.column < *x1)
                {
                    view.select_tab(i as i64);
                    view.on_tabs = true;
                }
            } else if view.body_area.contains(pos) {
                view.on_tabs = false;
            }
        }
        MouseEventKind::ScrollDown => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_LINES),
        _ => return,
    }
    app.dirty = true;
}

/// Enter: the editor opens on the focused file, embedded where the preview
/// was — the modal and its tabs stay put underneath, and quitting the
/// editor (or Ctrl+Q) lands back on the strip with the file re-read.
fn open_in_editor(app: &mut App) {
    let Some(Overlay::FileTabs(view)) = &app.overlay else {
        return;
    };
    let Some(tab) = view.selected() else {
        return;
    };
    let (root, editor) = (view.root.clone(), view.editor.clone());
    let path = tab.path.to_string_lossy().into_owned();
    // Size from the last-drawn body; the post-draw sync corrects it.
    let body = view.body_area;
    let size = if crate::event_loop::pane_usable(body) {
        (body.width, body.height)
    } else {
        crate::event_loop::vim_size_guess(app)
    };
    if crate::event_loop::spawn_editor_modal(app, &editor, &root, &path, 1, size) {
        if let Some(vim) = &mut app.vim {
            vim.embedded = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    fn text_of(line: &[(TokenKind, String)]) -> String {
        line.iter().map(|(_, t)| t.as_str()).collect()
    }

    fn view_in(app: &App) -> &FileTabsView {
        match &app.overlay {
            Some(Overlay::FileTabs(v)) => v,
            other => panic!("expected the file tabs, got {other:?}"),
        }
    }

    fn key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        handle_key(app, KeyEvent::new(code, mods));
    }

    #[test]
    fn labels_are_checkout_relative_or_the_file_name_and_cut_from_the_front() {
        let root = PathBuf::from("/repo");
        assert_eq!(
            label_for(&root, Path::new("/repo/docs/keys.md")),
            "docs/keys.md"
        );
        assert_eq!(
            label_for(&root, Path::new("/elsewhere/notes.md")),
            "notes.md"
        );
        let long = format!("/repo/{}/README.md", "a".repeat(40));
        let label = label_for(&root, Path::new(&long));
        assert_eq!(label.chars().count(), MAX_LABEL_CHARS);
        assert!(
            label.starts_with('…') && label.ends_with("aaa/README.md"),
            "{label}"
        );
    }

    #[test]
    fn opens_on_the_strip_previewing_the_first_file_and_tabs_wrap() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.md", "# alpha\nbody\n");
        let b = write(dir.path(), "b.rs", "fn main() {}\n");
        let mut view = FileTabsView::new(dir.path().to_path_buf(), "vi".into(), vec![a, b]);
        assert!(view.on_tabs);
        assert_eq!(view.tab, 0);
        assert!(view.preview_is_file);
        assert_eq!(text_of(&view.preview_lines[0]), "# alpha");
        view.select_tab(-1);
        assert_eq!(
            (view.tab, text_of(&view.preview_lines[0]).as_str()),
            (1, "fn main() {}")
        );
        view.select_tab(2);
        assert_eq!(view.tab, 0, "wraps past the last tab");
    }

    #[test]
    fn a_missing_file_previews_the_error_without_a_gutter() {
        let view = FileTabsView::new(
            "/tmp".into(),
            "vi".into(),
            vec!["/tmp/definitely-not-here-nebula.md".into()],
        );
        assert!(!view.preview_is_file);
        assert!(text_of(&view.preview_lines[0]).starts_with("couldn't read file"));
    }

    #[test]
    fn esc_backs_out_a_level_and_up_off_the_top_returns_to_the_strip() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.md", &"line\n".repeat(50));
        let mut app = App::new();
        let mut view = FileTabsView::new(dir.path().to_path_buf(), "vi".into(), vec![a]);
        view.view_height = 10; // as a draw would have written back
        app.overlay = Some(Overlay::FileTabs(view));

        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert!(!view_in(&app).on_tabs, "↓ drops into the preview");
        key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(view_in(&app).scroll, 6);
        key(&mut app, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(view_in(&app).scroll, 40, "clamped to the last page");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(
            view_in(&app).on_tabs,
            "Esc from the preview lands on the strip"
        );
        assert!(app.overlay.is_some());

        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        key(&mut app, KeyCode::Home, KeyModifiers::NONE);
        key(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert!(view_in(&app).on_tabs, "↑ off the top steps onto the strip");

        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none(), "Esc from the strip closes");
    }

    #[test]
    fn tab_keys_and_digits_switch_tabs_from_either_level() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (1..=3)
            .map(|i| write(dir.path(), &format!("f{i}.md"), &format!("file {i}\n")))
            .collect();
        let mut app = App::new();
        app.overlay = Some(Overlay::FileTabs(FileTabsView::new(
            dir.path().to_path_buf(),
            "vi".into(),
            files,
        )));
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(view_in(&app).tab, 1);
        key(&mut app, KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(view_in(&app).tab, 2);
        key(&mut app, KeyCode::Char('9'), KeyModifiers::NONE);
        assert_eq!(view_in(&app).tab, 2, "out-of-range digit is ignored");
        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        let view = view_in(&app);
        assert_eq!(
            (view.tab, view.on_tabs),
            (1, false),
            "←/→ work from the preview too"
        );
        assert_eq!(text_of(&view.preview_lines[0]), "file 2");
    }

    #[test]
    fn a_click_on_a_tab_label_switches_and_the_wheel_scrolls() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.md", &"x\n".repeat(30));
        let b = write(dir.path(), "b.md", "bee\n");
        let mut app = App::new();
        let mut view = FileTabsView::new(dir.path().to_path_buf(), "vi".into(), vec![a, b]);
        view.area = Rect::new(10, 5, 80, 30);
        view.tab_hits = vec![(12, 18), (19, 25)];
        view.body_area = Rect::new(11, 8, 78, 20);
        view.view_height = 20;
        app.overlay = Some(Overlay::FileTabs(view));
        let click = |app: &mut App, col: u16, row: u16| {
            handle_mouse(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: col,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                Position::new(col, row),
            );
        };
        click(&mut app, 20, 6);
        assert_eq!(view_in(&app).tab, 1, "the second label");
        click(&mut app, 20, 15);
        assert!(
            !view_in(&app).on_tabs,
            "a click in the body focuses the preview"
        );
        click(&mut app, 13, 6);
        assert!((view_in(&app).tab, view_in(&app).on_tabs) == (0, true));
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 20,
                row: 15,
                modifiers: KeyModifiers::NONE,
            },
            Position::new(20, 15),
        );
        assert_eq!(view_in(&app).scroll, WHEEL_LINES as u16);
    }
}
