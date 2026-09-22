//! True end-to-end TUI test: runs the real `nebula` binary inside a PTY,
//! sends literal keystrokes, and parses the rendered frames with vt100 —
//! asserting what a user would actually see on screen, including which row
//! is highlighted and which panel has focus.
//!
//! Flow under test:
//!   add two projects → Tab-walk focus → create two worktrees →
//!   j/k selection between worktrees → Enter into the sessions panel →
//!   create an agent (auto-attach) → per-worktree session isolation →
//!   j/k toggling between projects updates the worktree panel.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const COLS: u16 = 120;
const ROWS: u16 = 36;
const WAIT: Duration = Duration::from_secs(20);
/// Sleep between polls of the screen or a child.
const POLL_STEP: Duration = Duration::from_millis(50);

// Raw key bytes as the PTY sees them — the name replaces a trailing comment.
const ENTER: &[u8] = b"\r";
const TAB: &[u8] = b"\t";
const ESC: &[u8] = &[0x1b];
const DOWN: &[u8] = b"\x1b[B";
const CTRL_Q: &[u8] = &[0x11];
const CTRL_R: &[u8] = &[0x12];

/// A row only the PROJECT's own menu carries.
const PROJECT_MENU_ROW: &str = "Remove from list";
/// Terminal pane input-locked: keys forward to the PTY. The footer spells
/// chords the compact way `KeyChord::display` does — `^q`, not `Ctrl+q`.
const FOOTER_TERMINAL_LOCKED: &str = "^q: sessions";

struct TuiHarness {
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    runtime_dir: PathBuf,
    data_dir: PathBuf,
    _repos: tempfile::TempDir,
}

impl TuiHarness {
    fn spawn() -> Self {
        Self::spawn_with_env(&[])
    }

    /// `spawn`, plus environment overrides for the TUI process — used to put
    /// a stub `gh` on PATH so the pull-request row can be driven without a
    /// GitHub account.
    fn spawn_with_env(extra_env: &[(&str, String)]) -> Self {
        // Socket paths must stay under SUN_LEN (~104 bytes) — keep the
        // runtime dir short. Tests share one process, so a per-harness
        // sequence keeps each test on its own daemon.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let runtime_dir = PathBuf::from(format!("/tmp/nebtui-rt-{pid}-{seq}"));
        let data_dir = PathBuf::from(format!("/tmp/nebtui-data-{pid}-{seq}"));
        let _ = std::fs::remove_dir_all(&runtime_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        let repos = tempfile::tempdir().unwrap();

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_nebula"));
        cmd.env(nebula_core::env::RUNTIME_DIR, &runtime_dir);
        cmd.env(nebula_core::env::DATA_DIR, &data_dir);
        cmd.env(nebula_core::env::AGENT_CMD, "/bin/sh"); // stand-in for claude
        cmd.env(nebula_core::env::WORKTREE_SYNC_MS, "100"); // fast external-change pickup
        cmd.env(nebula_core::env::UPDATE_CHECK_SECS, "0"); // the footer must not depend on GitHub
        cmd.env(nebula_core::env::LOG, "debug");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("TERM", "xterm-256color");
        // Agent/CI shells often export NO_COLOR; crossterm then strips the
        // reverse/bold attrs wait_for_selected relies on.
        cmd.env_remove("NO_COLOR");
        cmd.env_remove("FORCE_COLOR");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.cwd(repos.path());
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer = pty.master.take_writer().unwrap();
        // Keep the master alive for the whole test (dropping it hangs up the
        // TUI's tty); leak is fine in a test process.
        std::mem::forget(pty.master);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        {
            let parser = parser.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    parser.lock().unwrap().process(&buf[..n]);
                }
            });
        }

        Self {
            writer,
            parser,
            child,
            runtime_dir,
            data_dir,
            _repos: repos,
        }
    }

    /// A committed git repo named `name` (fresh `git init` + one commit —
    /// worktrees need a HEAD to branch from).
    fn make_repo(&self, name: &str) -> PathBuf {
        let repo = self._repos.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| repo_git(&repo, args);
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@nebula.dev"]);
        git(&["config", "user.name", "nebula-test"]);
        std::fs::write(repo.join(".keep"), "").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "init"]);
        repo
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn type_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    fn screen_text(&self) -> String {
        let parser = self.parser.lock().unwrap();
        screen_to_text(parser.screen())
    }

    /// Poll the rendered screen until `pred` holds; panic with a full screen
    /// dump on timeout.
    fn wait_for(&self, what: &str, pred: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            {
                let parser = self.parser.lock().unwrap();
                if pred(parser.screen()) {
                    return;
                }
            }
            if Instant::now() > deadline {
                let tui_log = std::fs::read_to_string(self.data_dir.join("state/tui.log"))
                    .unwrap_or_default();
                let tail: String = tui_log
                    .lines()
                    .rev()
                    .take(60)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!(
                    "timed out waiting for: {what}\n--- screen ---\n{}\n--- tui.log tail ---\n{tail}",
                    self.screen_text()
                );
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn wait_for_text(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?}"), |s| {
            screen_to_text(s).contains(needle)
        });
    }

    /// Is `needle` on screen within `window`? Unlike `wait_for_text` this
    /// answers false instead of failing — for a flash that has to be let
    /// go of, and for asserting that something never comes up at all.
    fn try_wait_for_text(&self, needle: &str, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        loop {
            if screen_to_text(&self.parser.lock().unwrap().screen()).contains(needle) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn wait_for_gone(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?} to disappear"), |s| {
            !screen_to_text(s).contains(needle)
        });
    }

    /// Wait until the row containing `needle` renders with the selection
    /// fill (the raised `sel_bg` / `sel_bg_dim` background bar).
    fn wait_for_selected(&self, needle: &str) {
        self.wait_for(&format!("row {needle:?} selected (filled)"), |s| {
            row_is_selected(s, needle)
        });
    }
}

impl Drop for TuiHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Stop the auto-spawned daemon and clean the short-lived dirs.
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_nebula"))
            .arg("kill")
            .env(nebula_core::env::RUNTIME_DIR, &self.runtime_dir)
            .env(nebula_core::env::DATA_DIR, &self.data_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

fn screen_to_text(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for row in 0..rows {
        for col in 0..cols {
            match screen.cell(row, col) {
                Some(cell) => {
                    let contents = cell.contents();
                    if contents.is_empty() {
                        out.push(' ');
                    } else {
                        out.push_str(contents);
                    }
                }
                None => out.push(' '),
            }
        }
        out.push('\n');
    }
    out
}

fn row_is_selected(screen: &vt100::Screen, needle: &str) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        let mut line = String::new();
        for col in 0..cols {
            let contents = screen
                .cell(row, col)
                .map(|c| c.contents())
                .unwrap_or_default();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(contents);
            }
        }
        if let Some(at) = line.find(needle) {
            // Selection paints the row with the raised fill: indexed 237 in
            // the focused panel, 235 in unfocused ones (theme sel_bg /
            // sel_bg_dim). Only the needle's own panel band counts — the
            // panels share screen lines, and a selected pill in the next
            // column over used to pass this check for a row it had nothing
            // to do with.
            let at = line[..at].chars().count();
            let chars: Vec<char> = line.chars().collect();
            let band_start = chars[..at]
                .iter()
                .rposition(|&c| c == '│')
                .map_or(0, |i| i + 1);
            let band_end = chars[at..]
                .iter()
                .position(|&c| c == '│')
                .map_or(chars.len(), |i| at + i);
            let filled = (band_start..band_end).any(|col| {
                matches!(
                    screen.cell(row, col as u16).map(|c| c.bgcolor()),
                    Some(vt100::Color::Idx(237)) | Some(vt100::Color::Idx(235))
                )
            });
            if filled {
                return true;
            }
        }
    }
    false
}

fn add_project(tui: &mut TuiHarness, path: &Path, expect_name: &str) {
    tui.send(b"o");
    tui.wait_for_text("Open project");
    tui.type_str(&path.to_string_lossy());
    tui.send(ENTER);
    // The prompt must close before asserting grid content — otherwise the
    // overlay's own text can satisfy the wait (stale-frame race).
    tui.wait_for_gone("Open project");
    // Nothing opens over the grid — adding the first project no more than
    // any other. Callers land on the grid because nothing covered it.
    assert!(
        !tui.try_wait_for_text("what should the agent do?", Duration::from_millis(600)),
        "no modal may open on its own",
    );
    tui.wait_for_text(expect_name);
}

/// The project's own menu, where its verbs live: Esc lets the card under
/// the cursor go, and `m` with nothing selected is the project's menu.
/// The pause keeps the two keys apart — an ESC with a letter hard behind
/// it in the same read parses as Alt+letter.
fn open_project_menu(tui: &mut TuiHarness) {
    tui.send(ESC);
    std::thread::sleep(Duration::from_millis(150));
    tui.send(b"m");
    tui.wait_for_text(PROJECT_MENU_ROW);
}

/// Walk the open menu down to the row that reads `label`, and pick it.
fn choose_menu_row(tui: &mut TuiHarness, label: &str) {
    for _ in 0..12 {
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            if row_is_selected(tui.parser.lock().unwrap().screen(), label) {
                tui.send(ENTER);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        tui.send(b"j");
    }
    panic!(
        "menu row {label:?} never came under the cursor:\n{}",
        tui.screen_text()
    );
}

/// A session in the selected checkout, started from the box the way the
/// grid starts one, then stepped into so the pane has the keys. `task` is
/// typed into the box — the grid's box will not launch on an empty one.
fn start_session(tui: &mut TuiHarness, task: &str) {
    tui.send(b"p");
    tui.wait_for_text("what should the agent do?");
    tui.type_str(task);
    tui.send(ENTER);
    tui.wait_for_gone("what should the agent do?");
    // The launch lands the card without taking the pane; Enter steps into
    // it, which is where the keys have to be to type at the agent.
    tui.wait_for_text("agent-1");
    tui.send(ENTER);
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
}

fn repo_git(repo: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed in {}", repo.display());
}

#[test]
fn tui_help_modal_grouped_keymap() {
    let mut tui = TuiHarness::spawn();
    tui.wait_for_text("create your first project");

    // The grouped two-column keymap: every section header on screen at
    // once (the old single list clipped its tail on short terminals).
    tui.send(b"?");
    tui.wait_for_text("NAVIGATE & SEARCH");
    tui.wait_for_text("PROJECTS");
    tui.wait_for_text("WORKTREES");
    tui.wait_for_text("SESSIONS");
    tui.wait_for_text("TERMINAL & MOUSE");
    tui.wait_for_text("GENERAL");

    tui.send(ESC); // closes
    tui.wait_for_gone("NAVIGATE & SEARCH");
}

/// `nebula open` typed into a live session's shell — the stand-in agent is
/// `/bin/sh`, holding the AGENT ENV a real CLI would — raises the FILE
/// TABS in the TUI attached to that daemon: one tab per file, the first
/// previewed. Ctrl+Q from the strip closes it and hands the pane back.
#[test]
fn nebula_open_from_inside_a_session_raises_the_file_tabs() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("open-proj");
    let alpha = repo.join("alpha.md");
    let beta = repo.join("beta.rs");
    std::fs::write(&alpha, "# alpha opened from the session\n").unwrap();
    std::fs::write(&beta, "fn beta() {}\n").unwrap();

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "open-proj");

    // ---- an agent (the stand-in shell), stepped into and locked ----
    start_session(&mut tui, "hello");

    // ---- what the model runs, typed at the shell inside the session ----
    tui.type_str(&format!(
        "{} open {} {}",
        env!("CARGO_BIN_EXE_nebula"),
        alpha.display(),
        beta.display()
    ));
    tui.send(ENTER);
    tui.wait_for_text("Open files (2)");
    // The preview is the file itself — text the shell never echoed.
    tui.wait_for_text("alpha opened from the session");
    tui.wait_for_text("Enter: edit in");

    // ---- → moves to the next tab and its preview ----
    tui.send(b"\x1b[C");
    tui.wait_for_text("fn beta() {}");

    // ---- Ctrl+Q from the strip closes; the locked pane is back ----
    tui.send(CTRL_Q);
    tui.wait_for_gone("Open files (2)");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
}

/// Renaming a project is a label change, and an empty name undoes it.
/// Drives the real binary: the row picks up the new label with the folder
/// name hanging off a `└` underneath, then clearing the field puts the row
/// back on the folder's own name with nothing under it.
#[test]
fn tui_project_rename_shows_the_folder_and_empty_undoes_it() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("acme-repo");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "acme-repo");

    // ---- rename: the tab takes the label ----
    open_project_menu(&mut tui);
    choose_menu_row(&mut tui, "Rename");
    tui.wait_for_text("Rename project");
    // The field is prefilled with the current name; clear it first.
    tui.send(b"\x15"); // ^u
    tui.type_str("Acme API");
    tui.send(b"\r");
    tui.wait_for_gone("Rename project");
    tui.wait_for_text("Acme API");

    // ---- undo: an empty name puts the tab back on the folder name ----
    open_project_menu(&mut tui);
    choose_menu_row(&mut tui, "Rename");
    tui.wait_for_text("Rename project");
    tui.send(b"\x15"); // ^u clears the prefill
    tui.send(b"\r");
    tui.wait_for_gone("Rename project");
    // The chosen label is gone — the folder name is the card again,
    // exactly as a freshly added project renders.
    tui.wait_for_gone("Acme API");
    tui.wait_for_text("acme-repo");
}

/// Manual LINK creation is intentionally absent: Shift+L is unbound and
/// the HELP OVERLAY offers no attach-link action.
#[test]
fn tui_manual_link_add_is_unavailable() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("link-proj");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "link-proj");

    tui.send(b"L");
    tui.send(b"?");
    // If Shift+L still opened a prompt, this `?` would type into it instead
    // of opening HELP OVERLAY, so this heading proves the key was a no-op.
    tui.wait_for_text("NAVIGATE & SEARCH");
    tui.wait_for_gone("attach a link");
}

/// The ISSUES MODAL opens on rows fetched before `i` is ever pressed. A stub
/// `gh` on PATH answers `issue list` with one issue and records each call:
/// the record appears with no key sent — selecting the project is what
/// asks — and the modal then opens on that answer without a second ask.
#[test]
fn tui_issues_are_prefetched_before_the_modal_opens() {
    let stub_bin = tempfile::tempdir().unwrap();
    let calls = stub_bin.path().join("issue-list-calls");
    let gh = stub_bin.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            concat!(
                "#!/bin/sh\n",
                "case \"$1 $2\" in\n",
                "  'issue list') echo x >> '{calls}'; printf '%s' '[{{\"number\":15,",
                "\"title\":\"Fix login redirect\",\"url\":\"https://github.com/o/r/issues/15\",",
                "\"author\":{{\"login\":\"webdevcody\"}},\"createdAt\":\"2026-09-10T12:00:00Z\",",
                "\"updatedAt\":\"2026-09-11T12:00:00Z\",\"labels\":[],\"body\":\"Login bounces.\"}}]' ;;\n",
                "  'issue view') printf '%s' '{{\"url\":\"https://github.com/o/r/issues/15\",\"comments\":[]}}' ;;\n",
                "  *) exit 1 ;;\n",
                "esac\n",
            ),
            calls = calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut tui = TuiHarness::spawn_with_env(&[("PATH", path)]);
    let repo = tui.make_repo("issues-proj");
    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "issues-proj");

    // No `i` yet: the list is asked for because the project is selected.
    let deadline = Instant::now() + WAIT;
    while !calls.exists() {
        assert!(
            Instant::now() < deadline,
            "gh issue list never ran in the background\n--- screen ---\n{}",
            tui.screen_text()
        );
        std::thread::sleep(POLL_STEP);
    }

    // The modal opens on the prefetched row — and spends no second process
    // on a list that just landed (or is still landing).
    tui.send(b"i");
    tui.wait_for_text("Issues — issues-proj (1)");
    tui.wait_for_text("#15 Fix login redirect");
    let asks = std::fs::read_to_string(&calls).unwrap().lines().count();
    assert_eq!(asks, 1, "opening on a fresh list asks GitHub again");
    tui.send(ESC);
    tui.wait_for_gone("Issues — issues-proj");
}

/// `E` in the ISSUES MODAL edits the issue in place: the reading pane
/// becomes a form on the row's title and description, and Enter sends both
/// as one `gh issue edit` — the title on argv, the description on stdin —
/// then puts the reading pane back on the new text. A stub `gh` on PATH
/// answers the list and records the edit it is sent.
#[test]
fn tui_issues_modal_edits_the_issue_in_place() {
    // A stub `gh` that answers the list from what the last edit sent it,
    // as GitHub would, and records each edit it is sent.
    let stub_bin = tempfile::tempdir().unwrap();
    let edits = stub_bin.path().join("calls");
    let gh = stub_bin.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            concat!(
                "#!/bin/sh\n",
                "dir='{dir}'\n",
                "case \"$1 $2\" in\n",
                "  'issue list')\n",
                "    title='Fix login redirect'; body='Login bounces.'\n",
                "    [ -f \"$dir/title\" ] && title=$(cat \"$dir/title\")\n",
                "    [ -f \"$dir/body\" ] && body=$(cat \"$dir/body\")\n",
                "    printf '[{{\"number\":15,\"title\":\"%s\",\"url\":\"https://github.com/o/r/issues/15\",",
                "\"author\":{{\"login\":\"webdevcody\"}},\"createdAt\":\"2026-09-10T12:00:00Z\",",
                "\"updatedAt\":\"2026-09-11T12:00:00Z\",\"labels\":[],\"body\":\"%s\"}}]' \"$title\" \"$body\" ;;\n",
                "  'issue view') printf '%s' '{{\"url\":\"https://github.com/o/r/issues/15\",\"comments\":[]}}' ;;\n",
                "  'issue edit')\n",
                "    echo \"argv: $*\" >> \"$dir/calls\"\n",
                "    printf '%s' \"${{4#--title=}}\" > \"$dir/title\"\n",
                "    cat > \"$dir/body\"\n",
                "    printf 'stdin: ' >> \"$dir/calls\"; cat \"$dir/body\" >> \"$dir/calls\"; echo >> \"$dir/calls\" ;;\n",
                "  *) exit 1 ;;\n",
                "esac\n",
            ),
            dir = stub_bin.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut tui = TuiHarness::spawn_with_env(&[("PATH", path)]);
    let repo = tui.make_repo("issues-proj");
    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "issues-proj");
    tui.send(b"i");
    tui.wait_for_text("#15 Fix login redirect");

    // The form opens on the row's text, caret at the end of the title.
    tui.send(b"E");
    tui.wait_for_text("Edit issue #15");
    tui.wait_for_text("Title  Fix login redirect");
    tui.send(b"!");
    tui.wait_for_text("Fix login redirect!");
    tui.send(TAB);
    tui.send(b" Again.");
    tui.wait_for_text("Login bounces. Again.");

    // Enter sends the edit and the pane comes back on the new text.
    tui.send(ENTER);
    tui.wait_for_text("issue #15 updated");
    tui.wait_for_gone("Edit issue #15");
    tui.wait_for_text("#15 Fix login redirect!");
    let sent = std::fs::read_to_string(&edits).unwrap();
    assert!(
        sent.contains("argv: issue edit 15 --title=Fix login redirect! --body-file -"),
        "{sent}"
    );
    assert!(sent.contains("stdin: Login bounces. Again."), "{sent}");

    // Esc from the form drops the draft and keeps the modal.
    tui.send(b"E");
    tui.wait_for_text("Edit issue #15");
    tui.send(ESC);
    tui.wait_for_gone("Edit issue #15");
    tui.wait_for_text("Issues — issues-proj");
    tui.send(ESC);
    tui.wait_for_gone("Issues — issues-proj");
}

#[test]
fn tui_git_diff_modal() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("diff-proj");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "diff-proj");
    // The root worktree row must exist before g has anything to diff.

    // Dirty the checkout: one tracked modification, one untracked file.
    std::fs::write(repo.join(".keep"), "tracked change\n").unwrap();
    std::fs::write(repo.join("hello.txt"), "hello world\n").unwrap();

    // No wait on a changed-file count: it rides the session cards, and
    // this project has none. `g` reads the checkout for itself.

    // ---- open the modal; the selected file's diff renders ----
    tui.send(b"g");
    tui.wait_for_text("Files (2)");
    // Status is path-ordered, so .keep (modified) is selected first.
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");

    // ---- Ctrl+r marks .keep reviewed: it sinks below hello.txt and the
    // selection auto-advances to the next file, loading its diff ----
    tui.send(CTRL_R);
    tui.wait_for_text("· 1✓"); // files-panel title counts the mark
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+hello world");

    // ---- Down reaches the reviewed zone; Ctrl+r unmarks .keep, which
    // pops back to the top of the list and stays selected ----
    tui.send(DOWN);
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");
    tui.send(CTRL_R);
    tui.wait_for_gone("· 1✓");

    // ---- arrow to the untracked file ----
    tui.send(DOWN);
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+hello world");

    // ---- type-to-filter narrows the list and reselects the top match ----
    tui.type_str("kee");
    tui.wait_for_text("Files (1/2)");
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");
    tui.send(ESC); // first clears the filter, not the modal
    tui.wait_for_text("Files (2)");

    // ---- the modal blocks other interaction ----
    // n would open the box on the grid; inside the modal it feeds the
    // filter instead (verified after close — stale-frame convention).
    tui.send(b"n");
    tui.wait_for_text("no matches");
    tui.send(ESC); // clears the filter…
    tui.wait_for_text("Files (2)"); // (also keeps the two Escs from coalescing)
    tui.send(ESC); // …and the second closes the modal
    tui.wait_for_gone("Files (2)");
    assert!(
        !tui.screen_text().contains("what should the agent do?"),
        "modal swallowed n\n--- screen ---\n{}",
        tui.screen_text()
    );

    // ---- clean tree flashes instead of opening ----
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-m", "wip"]);
    tui.send(b"g");
    tui.wait_for_text("no changes in main");
}

/// The BRANCH SWITCHER end to end: `c` lists the repo's branches, typing
/// narrows them, `Enter` moves the root checkout on disk and the flash says
/// where it landed; a dirty checkout stops on the prompt instead, where `s`
/// stashes the changes under a named entry and switches.
#[test]
fn tui_branch_switcher_moves_the_root_checkout() {
    let head = |repo: &Path| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("switch-proj");
    repo_git(&repo, &["branch", "feature-login"]);
    repo_git(&repo, &["branch", "release-2"]);

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "switch-proj");

    // ---- a clean checkout switches on Enter ----
    tui.send(b"c");
    tui.wait_for_text("Switch branch — switch-proj");
    tui.wait_for_text("release-2");
    tui.type_str("login");
    tui.wait_for_gone("release-2");
    tui.send(ENTER);
    tui.wait_for_text("⌂ root is on feature-login");
    tui.wait_for_gone("Switch branch —");
    assert_eq!(head(&repo), "feature-login");

    // ---- a dirty one asks first; s stashes and switches ----
    std::fs::write(repo.join(".keep"), "edited\n").unwrap();
    tui.send(b"c");
    tui.wait_for_text("Switch branch — switch-proj");
    tui.wait_for_text("on feature-login");
    // The switch dropped the cached listing: Enter needs the fresh one.
    tui.wait_for_text("release-2");
    tui.type_str("main");
    tui.send(ENTER);
    tui.wait_for_text("how should they travel to main?");
    assert_eq!(
        head(&repo),
        "feature-login",
        "nothing moves before the answer"
    );
    tui.send(b"s");
    tui.wait_for_text("⌂ root is on main");
    assert_eq!(head(&repo), "main");
    let stashes = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["stash", "list"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&stashes.stdout)
            .contains("nebula: feature-login before switching to main"),
        "{}",
        String::from_utf8_lossy(&stashes.stdout)
    );
}

/// An SGR mouse report as the terminal would send it: `button` (0 = left,
/// 32 = left held while moving) at 0-based `col`,`row`.
fn sgr_mouse(button: u16, col: u16, row: u16, release: bool) -> Vec<u8> {
    format!(
        "\x1b[<{button};{};{}{}",
        col + 1,
        row + 1,
        if release { 'm' } else { 'M' }
    )
    .into_bytes()
}

/// Where `needle` first appears on screen: (row, col), in cells.
fn find_text(screen: &vt100::Screen, needle: &str) -> Option<(u16, u16)> {
    screen_to_text(screen)
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            let at = line.find(needle)?;
            Some((row as u16, line[..at].chars().count() as u16))
        })
}

/// How far back the pane says it is scrolled — the `scroll N` tag in the
/// TERMINAL header — or 0 at the live tail.
fn scrolled(screen: &vt100::Screen) -> usize {
    let text = screen_to_text(screen);
    text.match_indices("scroll ")
        .filter_map(|(at, _)| {
            let digits: String = text[at + 7..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .max()
        .unwrap_or(0)
}

/// A drag-select past the pane's top edge scrolls the history under the
/// pointer on the event loop's own beat — with no further mouse report —
/// and the release copies rows that were never on screen together. A stub
/// clipboard tool on PATH catches the copy.
#[test]
fn tui_drag_past_the_pane_top_autoscrolls_and_copies_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let stub_bin = tempfile::tempdir().unwrap();
    let copied = stub_bin.path().join("copied");
    // Whichever tool this platform's copy reaches for.
    for tool in ["pbcopy", "xclip", "xsel", "wl-copy"] {
        let stub = stub_bin.path().join(tool);
        std::fs::write(&stub, format!("#!/bin/sh\ncat > {}\n", copied.display())).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut tui = TuiHarness::spawn_with_env(&[("PATH", path)]);
    let repo = tui.make_repo("drag-proj");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "drag-proj");

    start_session(&mut tui, "hello");

    // Sixty numbered rows out of the stand-in shell: twice the pane's height.
    tui.type_str("i=1; while [ $i -le 60 ]; do echo \"row $i\"; i=$((i+1)); done");
    tui.send(ENTER);
    tui.wait_for_text("row 60");

    // The pane's first content row is two below its TERMINAL header;
    // `row 58` sits near the bottom of the pane.
    let (header_row, content_top, row58, col58) = {
        let parser = tui.parser.lock().unwrap();
        let screen = parser.screen();
        let (header_row, _) = find_text(screen, "SESSION").expect("the pane header");
        let (row58, col58) = find_text(screen, "row 58").expect("row 58 on screen");
        (header_row, header_row + 2, row58, col58)
    };
    // The PANE is a third of the body under the GRID, so "well inside it"
    // is a handful of rows rather than half a screen.
    assert!(
        row58 > content_top + 2,
        "row 58 is well inside the pane (header {header_row})"
    );
    // Rows on screen above `row 58` at the press; the top one is
    // `row {58 - visible_above}`.
    let visible_above = usize::from(row58 - content_top);
    let off_screen_row = format!("row {}", 58 - visible_above - 3);
    assert!(!tui.screen_text().contains(&off_screen_row));

    // Press on the last character of `row 58`, drag up onto the pane's
    // top row, then one row further — onto the rule above it — and rest.
    tui.send(&sgr_mouse(0, col58 + 5, row58, false));
    tui.send(&sgr_mouse(32, col58, content_top, false));
    tui.send(&sgr_mouse(32, col58, content_top - 1, false));
    // The loop's beat scrolls the history under the resting pointer; the
    // header counts the lines.
    tui.wait_for("the pane to scroll back under the held drag", |s| {
        scrolled(s) >= 5
    });
    tui.wait_for_text(&off_screen_row);

    // Release there: the copy runs from rows above anything that was on
    // screen at the press down to `row 58`.
    tui.send(&sgr_mouse(0, col58, content_top - 1, true));
    tui.wait_for_text("copied");
    let deadline = Instant::now() + WAIT;
    while !std::fs::read_to_string(&copied).is_ok_and(|t| t.ends_with("row 58")) {
        assert!(
            Instant::now() < deadline,
            "the copy never reached the stub clipboard"
        );
        std::thread::sleep(POLL_STEP);
    }
    let text = std::fs::read_to_string(&copied).unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows.len() >= visible_above + 6,
        "rows above the screen at the press are in the copy: {} rows, {visible_above} visible above row 58\n{text}",
        rows.len()
    );
    assert!(text.contains(&off_screen_row), "{text}");
    // Every row is one the shell printed, in order.
    let first: usize = rows[0]
        .strip_prefix("row ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("a numbered row first: {:?}", rows[0]));
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(*row, format!("row {}", first + i), "{text}");
    }
}
