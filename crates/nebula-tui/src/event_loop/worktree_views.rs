//! Extracted event-loop helper section.

use super::*;

pub(crate) fn open_launch_repo(app: &mut App, out: &mut Vec<ClientRequest>) {
    match app.launch_repo.clone() {
        Some(path) => open_folder(app, path, out),
        None => open_prompt(app, PromptKind::AddProject),
    }
}

/// Open the folder at `path` as a project — the open-project prompt's
/// Enter and the SPLASH's. A folder that is already a project (it, a
/// checkout of it, or a folder inside either) opens that project instead
/// of asking the daemon to register it twice; one that does not exist yet
/// asks first whether to create it, and one in no git repository asks
/// whether to `git init` it.
pub(crate) fn open_folder(app: &mut App, path: std::path::PathBuf, out: &mut Vec<ClientRequest>) {
    if !path.exists() {
        app.overlay = Some(Overlay::Confirm(ConfirmDialog {
            title: "Create directory".into(),
            message: format!(
                "{} doesn't exist, would you like to create it?",
                path.display()
            ),
            action: PendingAction::CreateProjectDir(path),
            area: ratatui::layout::Rect::default(),
        }));
        return;
    }
    let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let known = canon
        .ancestors()
        .find_map(|dir| app.tree.project_at_path(dir))
        .map(|p| (p.id.clone(), p.name.clone()));
    if let Some((id, name)) = known {
        launcher::open_project(app, &id, out);
        app.flash = Some(format!("{name} is already a project — opened it"));
        return;
    }
    if canon.is_dir() && !in_git_repo(&canon) {
        app.overlay = Some(Overlay::Confirm(ConfirmDialog {
            title: "Not a git repository".into(),
            message: format!(
                "{} isn't a git repository — nebula projects are. Run git init in it?",
                path.display()
            ),
            action: PendingAction::InitProjectRepo(path),
            area: ratatui::layout::Rect::default(),
        }));
        return;
    }
    send_with(app, out, PendingIntent::SelectCreatedProject, |req_id| {
        ClientRequest::AddProject {
            req_id,
            path,
            name: None,
            create_missing: false,
        }
    });
}

/// Whether `dir` sits in a git repository — a `.git` (a directory, or the
/// file a linked worktree carries) in it or above it, as git itself looks.
/// A stat per ancestor, so it is cheap enough for a keypress; the daemon
/// asks git itself before it runs `git init`.
pub(crate) fn in_git_repo(dir: &std::path::Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// Open the selected repo's page on its git host (`G`). Any worktree
/// answers, since every checkout of a project shares one remote — so the
/// cursor's worktree decides, falling back to the project's own clone when
/// it has no worktrees yet or the one selected is gone from disk.
pub(crate) fn open_repo_in_browser(app: &mut App) {
    let root = app
        .selected_worktree()
        .map(|w| w.path.clone())
        .filter(|path| path.is_dir())
        .or_else(|| app.selected_project().map(|p| p.repo_path.clone()));
    let Some(root) = root else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    // Not open_link: this is a repo page, never a PR row to mark read.
    let open = move || match crate::remote::repo_url(&root) {
        Ok(url) if open_url(&url) => format!("opened {}", crate::app::pretty_url(&url)),
        Ok(url) => format!("couldn't open {url}"),
        Err(msg) => msg,
    };
    // Which page it is takes a `git remote get-url` to know: asked off the
    // loop, with the outcome flashed when it lands.
    match app.view_jobs.clone() {
        Some(jobs) => {
            app.flash = Some("opening the repository's page…".into());
            jobs.run(move || Some(crate::view_jobs::Answer::Flash(open())));
        }
        None => app.flash = Some(open()),
    }
}

/// A new Ghostty tab in the selected worktree's directory (`Shift+C`), or in the
/// project's own clone when it has no worktrees yet. `open -a` hands
/// Ghostty a folder, which it takes like one dropped on its Dock icon: a tab
/// in the front window under the default `macos-dock-drop-behavior =
/// new-tab`. Silent — no flash at all — when there is no Ghostty to hand it
/// to: off macOS, no Ghostty.app installed, or over ssh, where `open` would
/// reach the remote machine's screen instead of the one being looked at.
pub(crate) fn open_ghostty_tab(app: &mut App) {
    open_ghostty_tab_with(app, ghostty_app());
}

pub(crate) fn open_ghostty_tab_with(app: &mut App, ghostty: Option<std::path::PathBuf>) {
    let Some(ghostty) = ghostty.filter(|_| !app.is_remote) else {
        return;
    };
    let dir = app
        .selected_worktree()
        .map(|w| w.path.clone())
        .or_else(|| app.selected_project().map(|p| p.repo_path.clone()));
    let Some(dir) = dir else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    if !dir.is_dir() {
        app.flash = Some(format!("path missing on disk: {}", dir.display()));
        return;
    }
    app.flash = Some(if open_in_app(&ghostty, &dir) {
        format!("opened a Ghostty tab in {}", dir.display())
    } else {
        format!("couldn't open a Ghostty tab in {}", dir.display())
    });
}

/// Ghostty.app where macOS installs put it: `/Applications` for the DMG drag
/// and Homebrew's cask, `~/Applications` for a per-user drag.
pub(crate) fn ghostty_app() -> Option<std::path::PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let mut roots = vec![std::path::PathBuf::from("/")];
    roots.extend(std::env::var_os("HOME").map(std::path::PathBuf::from));
    ghostty_app_in(&roots)
}

pub(crate) fn ghostty_app_in(roots: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    roots
        .iter()
        .map(|root| root.join("Applications/Ghostty.app"))
        .find(|bundle| bundle.is_dir())
}

/// `open -a <bundle> <path>`: whether LaunchServices took the hand-off.
pub(crate) fn open_in_app(bundle: &std::path::Path, path: &std::path::Path) -> bool {
    if cfg!(test) {
        return true;
    }
    let mut open = std::process::Command::new("open");
    open.arg("-a").arg(bundle).arg(path);
    spawn_and_reap(open, "open in app")
}

/// `r` on the Worktrees panel: start the selected checkout's RUN COMMAND,
/// or stop it when it is already up.
pub(crate) fn toggle_run(app: &mut App, out: &mut Vec<ClientRequest>) {
    if app.selected_worktree_pr().is_some() {
        app.flash = Some("a pull request has no checkout to run — pick a worktree".into());
        return;
    }
    let Some(w) = app.selected_worktree().cloned() else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    toggle_run_in(app, &w, out);
}

/// Ask the DAEMON to start `worktree`'s run, or to stop it while it runs.
pub(crate) fn toggle_run_in(
    app: &mut App,
    worktree: &nebula_core::Worktree,
    out: &mut Vec<ClientRequest>,
) {
    if app.is_placeholder_worktree(&worktree.id) {
        app.flash = Some(WORKTREE_STILL_CREATING.into());
        return;
    }
    let start = !app.worktree_running(&worktree.id);
    let intent = PendingIntent::RunToggled {
        branch: worktree.branch.clone(),
        started: start,
    };
    let id = worktree.id.clone();
    send_with(app, out, intent, |req_id| {
        if start {
            ClientRequest::StartRun {
                req_id,
                worktree: id,
            }
        } else {
            ClientRequest::StopRun {
                req_id,
                worktree: id,
            }
        }
    });
}

/// `Shift+Enter` / `Shift+O`: fire the selected checkout's OPEN COMMAND.
pub(crate) fn open_selected_worktree(app: &mut App) {
    if app.selected_worktree_pr().is_some() {
        app.flash = Some("a pull request has no checkout to open — pick a worktree".into());
        return;
    }
    let Some(w) = app.selected_worktree().cloned() else {
        app.flash = Some(SELECT_CONTEXT_FIRST.into());
        return;
    };
    open_worktree(app, &w);
}

/// Why `Shift+Enter` has nothing to run, naming both places to put one —
/// the twin of the DAEMON's line for `r`.
const NO_OPEN_COMMAND: &str =
    "no open command for this worktree — set one in Settings (s) → Project, \
                               or add .nebula.json with {\"open\": \"open http://localhost:3000\"}";

/// Run `worktree`'s OPEN COMMAND once and say so: the project's **Open
/// command** setting (Settings → Project) when it is set, else the
/// checkout's `.nebula.json` `open`. The TUI runs it, not the DAEMON: it
/// opens a browser or an editor on the machine the user is sitting at.
pub(crate) fn open_worktree(app: &mut App, worktree: &nebula_core::Worktree) {
    let main = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == worktree.project_id)
        .map_or_else(|| worktree.path.clone(), |p| p.repo_path.clone());
    let command = match open_command_for(&worktree.path, &main) {
        Ok(command) => command,
        Err(msg) => {
            app.flash = Some(msg);
            return;
        }
    };
    app.flash = Some(match spawn_open_command(&command, &worktree.path) {
        Ok(()) => format!("↗ {command}"),
        Err(e) => format!("couldn't run {command}: {e}"),
    });
}

/// The OPEN COMMAND for a checkout, resolved the way the DAEMON resolves
/// `r`'s: the project's `open_command` setting, read fresh so an edit
/// applies on the next press, else the PROJECT FILE's `open` — the
/// worktree's own checkout, then the project's main one. Err is the footer
/// line: nothing set in either place, or a file that is there and won't
/// parse.
pub(crate) fn open_command_for(
    worktree: &std::path::Path,
    main: &std::path::Path,
) -> Result<String, String> {
    let setting = crate::config::Config::load().project(main).open_command;
    let setting = setting.trim();
    if !setting.is_empty() {
        return Ok(setting.to_string());
    }
    nebula_core::project_file::lookup(
        worktree,
        main,
        nebula_core::project_file::ProjectCommand::Open,
    )?
    .ok_or_else(|| NO_OPEN_COMMAND.to_string())
}

/// Start an OPEN COMMAND through `$SHELL -c` in `cwd`, kept off the TUI's
/// screen: no stdin, output discarded (a stray byte on the TUI's terminal
/// tears the frame), a process group of its own, reaped on a thread. A
/// failure to start is the caller's to show; a non-zero exit only logs,
/// since by then `open` has handed the URL over or said why not.
pub(crate) fn spawn_open_command(command: &str, cwd: &std::path::Path) -> std::io::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let shell = nebula_core::shell::user_shell();
    let mut child = Command::new(shell)
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let command = command.to_string();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => {
            tracing::warn!(%command, %status, "open command failed");
        }
        Err(err) => tracing::warn!(%command, %err, "open command not reaped"),
        Ok(_) => {}
    });
    Ok(())
}

/// The selected worktree's checkout — path and branch — for the modals that
/// read it. Flashes and returns None when no worktree is selected or its
/// path is gone from disk.
pub(crate) fn selected_checkout(app: &mut App) -> Option<(std::path::PathBuf, String)> {
    // Clone before touching app.overlay — selected_worktree borrows app.
    let Some((path, branch)) = app
        .selected_worktree()
        .map(|w| (w.path.clone(), w.branch.clone()))
    else {
        app.flash = Some("no worktree selected".into());
        return None;
    };
    if !path.is_dir() {
        app.flash = Some(format!("worktree path missing on disk: {}", path.display()));
        return None;
    }
    Some((path, branch))
}

/// Every tracked + untracked file of a checkout, plus the configured editor
/// command, for the finder and tree modals. Flashes and returns None when
/// git fails or the checkout has no files.
pub(crate) fn load_worktree_files(
    app: &mut App,
    path: &std::path::Path,
    branch: &str,
) -> Option<(Vec<String>, String)> {
    let files = match crate::git_diff::list_files(path) {
        Ok(files) => files,
        Err(msg) => {
            app.flash = Some(msg);
            return None;
        }
    };
    if files.is_empty() {
        app.flash = Some(format!("no files in {branch}"));
        return None;
    }
    let editor = crate::config::Config::load().editor_command();
    Some((files, editor))
}

/// `g`: the DIFF VIEWER on the selected checkout. The modal is up on this
/// keypress and its file list lands when `git status` answers
/// (`land_view_answer`) — it is that `git status`, the HEAD lookup and a
/// `git diff` per reviewed ✓ mark that the key used to wait on. The
/// reviewed marks `git_diff::read_listing` restores drop any that no
/// longer apply: `load_marks` already returns nothing when HEAD moved (a
/// commit resets the whole worktree), and a mark whose file left the change
/// list or whose diff text changed since it was approved is pruned — then
/// the pruned set is written back. Restored marks sink to the bottom, so
/// the modal opens on the first unreviewed file.
///
/// A checkout the changed-files badge already knows to be clean is told so
/// on the spot instead of being shown a modal that closes again; the badge
/// can be two seconds behind an agent, so git is still asked, and the
/// modal opens after all if it disagrees (`App::diff_probe`).
pub(crate) fn open_diff_view(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    let Some(jobs) = app.view_jobs.clone() else {
        // No loop to land an answer on (unit tests): read inline.
        match crate::git_diff::read_listing(&path) {
            Ok(listing) => show_diff_listing(app, path, branch, listing),
            Err(msg) => app.flash = Some(msg),
        }
        return;
    };
    let ticket = crate::view_jobs::ticket();
    let selected = app.selected_worktree().map(|w| w.id.clone());
    let known_clean =
        matches!(&app.git_changes, Some((id, Some(0))) if Some(id) == selected.as_ref());
    if known_clean {
        app.flash = Some(format!("no changes in {branch}"));
        app.diff_probe = Some((ticket, path.clone(), branch));
    } else {
        let mut view = DiffView::opening(path.clone(), branch, jobs.clone(), ticket);
        view.files_width = app.diff_files_width;
        if app.diff_tree {
            view.toggle_tree();
        }
        // The badge's last `git status` — two seconds old at most — is the
        // list to open on: the files are up on this keypress and the first
        // diff is being read while the `git status` below checks them, not
        // after it. `fill_view` reconciles the two when that lands.
        let polled = app
            .changed_files
            .as_ref()
            .filter(|(id, files)| Some(id) == selected.as_ref() && !files.is_empty());
        if let Some((_, files)) = polled {
            view.replace_files(files.clone());
            crate::git_diff::load_selected_diff(&mut view);
        }
        app.overlay = Some(Overlay::Diff(view));
    }
    jobs.run(move || {
        Some(crate::view_jobs::Answer::DiffListing {
            ticket,
            result: crate::git_diff::read_listing(&path),
        })
    });
}

/// Open the DIFF VIEWER on a listing already in hand — or say there is
/// nothing to show.
pub(crate) fn show_diff_listing(
    app: &mut App,
    path: std::path::PathBuf,
    branch: String,
    listing: crate::view_jobs::DiffListing,
) {
    if listing.files.is_empty() {
        app.flash = Some(format!("no changes in {branch}"));
        return;
    }
    let mut view = DiffView::new(path, branch, Vec::new(), true);
    view.jobs = app.view_jobs.clone();
    view.files_width = app.diff_files_width;
    crate::git_diff::fill_view(&mut view, listing);
    // After the marks: the tree opens on the first unreviewed file too.
    if app.diff_tree && view.toggle_tree() {
        crate::git_diff::load_selected_diff(&mut view);
    }
    app.overlay = Some(Overlay::Diff(view));
}

/// Fuzzy file finder over every tracked + untracked file of the selected
/// worktree (`f`). Same shell as `open_diff_view`: flash instead of opening
/// when there's no worktree, the path is gone, or git fails.
/// `Shift+H`: destinations remembered by `nebula ssh`, newest first. Opens even
/// when empty — the modal's hint is how the feature introduces itself.
pub(crate) fn open_hosts_picker(app: &mut App) {
    app.overlay = Some(Overlay::Hosts(crate::app::HostsView::new(
        crate::hosts::load(),
    )));
}

pub(crate) fn open_file_finder(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    // The modal is up on this keypress, taking what is typed; the list
    // lands when `git ls-files` answers (`land_view_answer`).
    if let Some(jobs) = app.view_jobs.clone() {
        let editor = crate::config::Config::load().editor_command();
        let ticket = request_worktree_files(&jobs, &path);
        app.overlay = Some(Overlay::Files(FileFinder::opening(
            path, branch, editor, ticket,
        )));
        return;
    }
    let Some((files, editor)) = load_worktree_files(app, &path, &branch) else {
        return;
    };
    app.overlay = Some(Overlay::Files(FileFinder::new(path, branch, editor, files)));
}

/// Ask for a checkout's file listing off the loop; the ticket is what the
/// modal opened ahead of it waits on.
pub(crate) fn request_worktree_files(jobs: &crate::view_jobs::Jobs, path: &std::path::Path) -> u64 {
    let ticket = crate::view_jobs::ticket();
    let root = path.to_path_buf();
    jobs.run(move || {
        Some(crate::view_jobs::Answer::Files {
            ticket,
            result: crate::git_diff::list_files(&root),
        })
    });
    ticket
}

/// Tree browser (`b`): full file tree of the selected worktree with a
/// content preview, filterable by file name. Same shell as `open_diff_view`:
/// flash instead of opening when there's no worktree, the path is gone, or
/// git fails.
pub(crate) fn open_tree_browser(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    // Up on this keypress; the tree lands when `git ls-files` answers.
    if let Some(jobs) = app.view_jobs.clone() {
        let editor = crate::config::Config::load().editor_command();
        let ticket = request_worktree_files(&jobs, &path);
        app.overlay = Some(Overlay::Tree(TreeBrowser::opening(
            path, branch, editor, jobs, ticket,
        )));
        return;
    }
    let Some((files, editor)) = load_worktree_files(app, &path, &branch) else {
        return;
    };
    app.overlay = Some(Overlay::Tree(TreeBrowser::new(path, branch, editor, files)));
}

/// Find-in-files (`F`): live `git grep` over the selected worktree; Enter
/// on a hit opens it in the editor modal. Same shell as `open_diff_view`.
pub(crate) fn open_grep_view(app: &mut App) {
    let Some((path, branch)) = selected_checkout(app) else {
        return;
    };
    let editor = crate::config::Config::load().editor_command();
    let mut view = GrepView::new(path, branch, editor);
    view.jobs = app.view_jobs.clone();
    app.overlay = Some(Overlay::Grep(view));
}

/// A BACKGROUND READ came back (`view_jobs`): hand it to the view that
/// asked, if that view is still the one on screen. Every answer carries
/// the ticket its view is waiting on, so one that outlived its modal — or
/// its query, or its cursor — is dropped by the view itself.
pub(crate) fn land_view_answer(app: &mut App, answer: crate::view_jobs::Answer) {
    use crate::view_jobs::Answer;
    match answer {
        Answer::Grep { ticket, result } => {
            if let Some(Overlay::Grep(view)) = &mut app.overlay {
                view.land(ticket, result);
            }
        }
        Answer::Files { ticket, result } => land_worktree_files(app, ticket, result),
        Answer::DiffListing { ticket, result } => land_diff_listing(app, ticket, result),
        Answer::DiffText {
            view: id,
            ticket,
            path,
            diff,
            prefetch,
        } => {
            if let Some(Overlay::Diff(view)) = &mut app.overlay {
                crate::git_diff::land_diff(view, id, ticket, &path, diff, prefetch);
            }
        }
        Answer::Preview { ticket, preview } => match &mut app.overlay {
            Some(Overlay::Tree(view)) => view.land_preview(ticket, *preview),
            Some(Overlay::FileTabs(view)) => view.land_preview(ticket, *preview),
            _ => {}
        },
        Answer::ClipboardViaTerminal { payload, flash } => {
            app.pending_clipboard = Some(payload);
            app.flash = Some(flash);
        }
        Answer::Flash(message) => app.flash = Some(message),
        Answer::ClientRss(bytes) => land_client_rss(app, bytes),
        Answer::Slow { ticket } => match &mut app.overlay {
            Some(Overlay::Diff(view)) => crate::git_diff::diff_slow(view, ticket),
            Some(Overlay::Tree(view)) => view.preview_slow(ticket),
            Some(Overlay::FileTabs(view)) => view.preview_slow(ticket),
            _ => {}
        },
    }
    app.dirty = true;
}

/// `git ls-files` came back for the FILE FINDER or the TREE BROWSER that
/// opened ahead of it. A checkout with nothing to list, or one git could
/// not list, closes the modal with the reason — what `f` and `b` used to
/// say instead of opening.
pub(crate) fn land_worktree_files(app: &mut App, ticket: u64, result: Result<Vec<String>, String>) {
    let branch = match &app.overlay {
        Some(Overlay::Files(finder)) if finder.listing == Some(ticket) => finder.branch.clone(),
        Some(Overlay::Tree(view)) if view.listing == Some(ticket) => view.branch.clone(),
        _ => return,
    };
    let files = match result {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            app.overlay = None;
            app.flash = Some(format!("no files in {branch}"));
            return;
        }
        Err(msg) => {
            app.overlay = None;
            app.flash = Some(msg);
            return;
        }
    };
    match &mut app.overlay {
        Some(Overlay::Files(finder)) => finder.set_files(files),
        Some(Overlay::Tree(view)) => view.set_files(files),
        _ => {}
    }
}

/// `git status` came back for a `g`: fill the DIFF VIEWER that opened ahead
/// of it — or close it, saying why, when there is nothing to show — or, for
/// the checkout that was told "no changes" off the badge, open it after all
/// when git found some and nothing else has taken the screen since.
pub(crate) fn land_diff_listing(
    app: &mut App,
    ticket: u64,
    result: Result<crate::view_jobs::DiffListing, String>,
) {
    let probe = match &app.diff_probe {
        Some((probed, ..)) if *probed == ticket => app.diff_probe.take(),
        _ => None,
    };
    if let Some((_, path, branch)) = probe {
        if let Ok(listing) = result {
            if !listing.files.is_empty() && app.overlay.is_none() && app.vim.is_none() {
                app.flash = None;
                show_diff_listing(app, path, branch, listing);
            }
        }
        return;
    }
    let branch = match &app.overlay {
        Some(Overlay::Diff(view)) if view.listing == Some(ticket) => view.branch.clone(),
        _ => return,
    };
    match result {
        Ok(listing) if !listing.files.is_empty() => {
            if let Some(Overlay::Diff(view)) = &mut app.overlay {
                crate::git_diff::fill_view(view, listing);
            }
        }
        Ok(_) => {
            app.overlay = None;
            app.flash = Some(format!("no changes in {branch}"));
        }
        Err(msg) => {
            app.overlay = None;
            app.flash = Some(msg);
        }
    }
}

/// Enter on a grep hit: spawn the editor at `path:line` inside the modal
/// terminal. With `close_finder_on_open` the grep overlay closes as the
/// editor opens, so quitting the editor is a single Esc; with it off the
/// overlay stays open underneath and quitting lands back on the results.
pub(crate) fn open_selected_hit_in_editor(app: &mut App) {
    let Some(Overlay::Grep(view)) = &app.overlay else {
        return;
    };
    let Some(hit) = view.selected_hit() else {
        return;
    };
    let (root, editor) = (view.root.clone(), view.editor.clone());
    let (path, line) = (hit.path.clone(), hit.line);
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let size = vim_size_guess(app);
    if spawn_editor_modal(app, &editor, &root, &path, line, size) {
        close_finder_behind_editor(app);
    }
}

/// Drop the finder overlay the editor was just launched from, when the
/// setting asks for it. Only called after a spawn actually succeeded — a
/// failed spawn (or a unit test with no reader channel) must leave the
/// results on screen rather than dismiss them for nothing.
pub(crate) fn close_finder_behind_editor(app: &mut App) {
    if crate::config::Config::load().close_finder_on_open {
        app.overlay = None;
    }
}

/// Boot `editor` on `file` at `line` (cwd `root`) into the editor modal at
/// grid `size`, replacing whatever it held; a spawn failure flashes. False
/// when nothing was spawned — the main loop isn't running (unit tests
/// without a channel) or the spawn failed.
pub(crate) fn spawn_editor_modal(
    app: &mut App,
    editor: &str,
    root: &std::path::Path,
    file: &str,
    line: u64,
    size: (u16, u16),
) -> bool {
    let Some(tx) = app.vim_tx.clone() else {
        return false;
    };
    let (cols, rows) = size;
    app.vim_generation += 1;
    match VimTerm::spawn_editor(editor, root, file, line, cols, rows, app.vim_generation, tx) {
        Ok(vim) => {
            app.vim = Some(vim);
            true
        }
        Err(msg) => {
            app.flash = Some(msg);
            false
        }
    }
}

/// Enter on a file-finder row: spawn the editor at the file's first line
/// inside the modal terminal. With `close_finder_on_open` the finder closes
/// as the editor opens, so quitting the editor is a single Esc; with it off
/// the finder stays open underneath and quitting lands back on the results.
/// Enter in the finder: a markdown file is read first — the FILE TABS'
/// rendered page takes the finder's place, and Enter there is the editor
/// — since a `.md` is usually opened to be read; anything else goes
/// straight to the editor modal.
pub(crate) fn open_selected_file(app: &mut App) {
    let Some(Overlay::Files(finder)) = &app.overlay else {
        return;
    };
    let Some(path) = finder.selected_path().map(str::to_string) else {
        return;
    };
    if crate::markdown::is_markdown_path(&path) {
        let root = finder.root.clone();
        let file = root.join(&path);
        crate::file_tabs::open(app, root, vec![file]);
        return;
    }
    open_selected_file_in_editor(app);
}

pub(crate) fn open_selected_file_in_editor(app: &mut App) {
    let Some(Overlay::Files(finder)) = &app.overlay else {
        return;
    };
    let Some(path) = finder.selected_path().map(str::to_string) else {
        return;
    };
    let (root, editor) = (finder.root.clone(), finder.editor.clone());
    // Size guess from the last-drawn body; the post-draw sync corrects it.
    let size = vim_size_guess(app);
    if spawn_editor_modal(app, &editor, &root, &path, 1, size) {
        close_finder_behind_editor(app);
    }
}

/// Enter on a tree-browser file row: spawn the editor embedded in the
/// preview pane — the pane becomes vim, keys flow to it, and quitting lands
/// back on the tree with the preview reloaded.
pub(crate) fn open_selected_tree_file_in_editor(app: &mut App) {
    let Some(Overlay::Tree(view)) = &app.overlay else {
        return;
    };
    let Some(path) = view
        .selected_node()
        .filter(|n| !n.is_dir)
        .map(|n| n.path.clone())
    else {
        return;
    };
    let (root, editor) = (view.root.clone(), view.editor.clone());
    // Size from the last-drawn preview pane; the post-draw sync corrects it.
    let preview = view.preview_area;
    let size = if pane_usable(preview) {
        (preview.width, preview.height)
    } else {
        vim_size_guess(app) // never drawn yet
    };
    if spawn_editor_modal(app, &editor, &root, &path, 1, size) {
        if let Some(vim) = &mut app.vim {
            vim.embedded = true;
        }
    }
}

/// Expected inner size of the editor modal before its first draw, derived
/// from the last-drawn body rect (`VIM_MODAL_PCT` of the frame, minus the
/// border). `sync_vim_size` trues it up after the real draw.
pub(crate) fn vim_size_guess(app: &App) -> (u16, u16) {
    let frame_w = app.body_area.width;
    let frame_h = app.body_area.height + 2; // + footer row and its padding
    let cols = (frame_w * ui::VIM_MODAL_PCT.0 / 100)
        .saturating_sub(2)
        .max(MIN_PANE_DIM);
    let rows = (frame_h * ui::VIM_MODAL_PCT.1 / 100)
        .saturating_sub(2)
        .max(MIN_PANE_DIM);
    (cols, rows)
}

/// ⌥click on a file path in the terminal pane: resolve it against the
/// attached session's worktree and open it in the editor modal at the
/// referenced line — or, for a markdown file, read it first in the FILE
/// TABS (the rendered page; Enter there is the editor), since a `.md` an
/// agent points at is there to be read. The line, if any, is the source's
/// and the page has no row for it.
pub(crate) fn open_file_link(app: &mut App, path: &str, line: Option<u64>) {
    let Some(root) = attached_worktree_root(app) else {
        app.flash = Some("no worktree for this session".into());
        return;
    };
    let Some(file) = resolve_file_link(&root, path) else {
        app.flash = Some(format!("file not found: {path}"));
        return;
    };
    if crate::markdown::is_markdown_path(&file) {
        // `join` leaves an absolute (`~/`-expanded) path as it is.
        let file = root.join(&file);
        crate::file_tabs::open(app, root, vec![file]);
        return;
    }
    let editor = crate::config::Config::load().editor_command();
    let size = vim_size_guess(app);
    spawn_editor_modal(app, &editor, &root, &file, line.unwrap_or(1), size);
}

/// Worktree root of the attached agent or shell; falls back to the
/// selected worktree when nothing is attached (or it isn't in the tree yet).
pub(crate) fn attached_worktree_root(app: &App) -> Option<std::path::PathBuf> {
    let worktree_id = app.term.as_ref().and_then(|t| match &t.sref {
        SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| &a.worktree_id),
        SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| &t.worktree_id),
    });
    worktree_id
        .and_then(|id| app.tree.worktrees.iter().find(|w| &w.id == id))
        .or_else(|| app.selected_worktree())
        .map(|w| w.path.clone())
}

/// Resolve a clicked path against the worktree: expand `~/`, try it as
/// printed, then with the git-diff `a/`/`b/` prefix stripped. Returns the
/// argument to hand the editor — relative paths stay relative, since the
/// editor runs with the worktree as cwd.
pub(crate) fn resolve_file_link(root: &std::path::Path, path: &str) -> Option<String> {
    let mut candidates = vec![path];
    for prefix in ["a/", "b/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            candidates.push(rest);
        }
    }
    for cand in candidates {
        let full = if let Some(rest) = cand.strip_prefix("~/") {
            nebula_core::env::home_dir()?.join(rest)
        } else {
            // join() with an absolute candidate yields the candidate.
            root.join(cand)
        };
        if full.is_file() {
            return Some(if cand.starts_with("~/") {
                full.to_string_lossy().into_owned()
            } else {
                cand.to_string()
            });
        }
    }
    None
}
