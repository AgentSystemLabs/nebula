//! True end-to-end TUI test: runs the real `nebula` binary inside a PTY,
//! sends literal keystrokes, and parses the rendered frames with vt100 —
//! asserting what a user would actually see on screen, including which row
//! is highlighted and which panel has focus.
//!
//! Flow under test:
//!   add two projects → Tab-cycle focus → create two worktrees →
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

// Distinct footer hints identify the focused panel on screen.
const FOOTER_PROJECTS: &str = "n/o: add";
const FOOTER_WORKTREES: &str = "n: new worktree";
const FOOTER_SESSIONS: &str = "n: agent";
/// Terminal pane focused but NOT input-locked (attached session).
const FOOTER_TERMINAL_FOCUSED: &str = "Enter: type into terminal";
/// Terminal pane input-locked: keys forward to the PTY.
const FOOTER_TERMINAL_LOCKED: &str = "Ctrl+q: panels";

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
        cmd.env("NEBULA_RUNTIME_DIR", &runtime_dir);
        cmd.env("NEBULA_DATA_DIR", &data_dir);
        cmd.env("NEBULA_AGENT_CMD", "/bin/sh"); // stand-in for claude
        cmd.env("NEBULA_WORKTREE_SYNC_MS", "100"); // fast external-change pickup
        cmd.env("NEBULA_LOG", "debug");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("TERM", "xterm-256color");
        // Agent/CI shells often export NO_COLOR; crossterm then strips the
        // reverse/bold attrs wait_for_selected relies on.
        cmd.env_remove("NO_COLOR");
        cmd.env_remove("FORCE_COLOR");
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
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?} failed in {}", repo.display());
        };
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
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_text(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?}"), |s| {
            screen_to_text(s).contains(needle)
        });
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

    /// Wait until `needle` no longer appears inside the Sessions panel's
    /// column band. Screen-wide checks would false-positive on the terminal
    /// pane title, which keeps naming the attached session while browsing
    /// other worktrees/projects.
    fn wait_for_sessions_row_gone(&self, needle: &str) {
        self.wait_for(&format!("sessions row {needle:?} to disappear"), |s| {
            !sessions_panel_contains(s, needle)
        });
    }
}

impl Drop for TuiHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Stop the auto-spawned daemon and clean the short-lived dirs.
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_nebula"))
            .arg("kill")
            .env("NEBULA_RUNTIME_DIR", &self.runtime_dir)
            .env("NEBULA_DATA_DIR", &self.data_dir)
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
                        out.push_str(&contents);
                    }
                }
                None => out.push(' '),
            }
        }
        out.push('\n');
    }
    out
}

/// True when `needle` appears within the Sessions panel's columns
/// (mirrors DEFAULT_PANEL_WIDTHS in nebula-tui/src/app.rs; the harness
/// starts with a fresh DB, so the panels are at their default widths).
fn sessions_panel_contains(screen: &vt100::Screen, needle: &str) -> bool {
    const SESSIONS_X: u16 = 20 + 22;
    const SESSIONS_W: u16 = 26;
    let (rows, cols) = screen.size();
    let right = SESSIONS_X.saturating_add(SESSIONS_W).min(cols);
    for row in 0..rows {
        let mut line = String::new();
        for col in SESSIONS_X..right {
            let contents = screen
                .cell(row, col)
                .map(|c| c.contents())
                .unwrap_or_default();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&contents);
            }
        }
        if line.contains(needle) {
            return true;
        }
    }
    false
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
                line.push_str(&contents);
            }
        }
        if line.contains(needle) {
            // Selection paints the row with the raised fill: indexed 237 in
            // the focused panel, 235 in unfocused ones (theme sel_bg /
            // sel_bg_dim).
            let filled = (0..cols).any(|col| {
                matches!(
                    screen.cell(row, col).map(|c| c.bgcolor()),
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
    tui.send(b"n");
    tui.wait_for_text("Add project");
    tui.type_str(&path.to_string_lossy());
    tui.send(b"\r");
    // The prompt must close before asserting panel content — otherwise the
    // overlay's own text can satisfy the wait (stale-frame race).
    tui.wait_for_gone("Add project");
    tui.wait_for_text(expect_name);
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

fn create_worktree(tui: &mut TuiHarness, branch: &str) {
    tui.send(b"n");
    tui.wait_for_text("New worktree");
    tui.type_str(branch);
    tui.send(b"\r");
    tui.wait_for_gone("New worktree");
    tui.wait_for_text(branch);
    // A fresh worktree auto-focuses the sessions panel (so `n` starts an
    // agent); hop back to Worktrees so callers stay panel-stable.
    tui.wait_for_text(FOOTER_SESSIONS);
    tui.send(b"\x1b[D"); // ← (h is the hosts picker)
    tui.wait_for_text(FOOTER_WORKTREES);
}

#[test]
fn tui_projects_worktrees_agents_navigation() {
    let mut tui = TuiHarness::spawn();
    let alpha = tui.make_repo("alpha-proj");
    let beta = tui.make_repo("beta-proj");

    // ---- boot: empty state, Projects focused ----
    tui.wait_for_text("create your first project");
    tui.wait_for_text(FOOTER_PROJECTS);

    // ---- add the first project via bash-style Tab completion ----
    // Type the repos dir + "al", press Tab: unique match completes to
    // "alpha-proj/" on screen.
    tui.send(b"n");
    tui.wait_for_text("Add project");
    tui.type_str(&format!("{}/al", alpha.parent().unwrap().display()));
    tui.send(b"\t");
    tui.wait_for_text("alpha-proj/");
    tui.send(b"\r");
    tui.wait_for_gone("Add project");
    tui.wait_for_text("alpha-proj");
    tui.wait_for_text("main ⌂ root"); // main checkout appears as the root row

    // The live directory browser: typing "…/T/.tmpX/" lists both repos as
    // rows (no Tab needed), then Esc cancels.
    tui.send(b"n");
    tui.wait_for_text("Add project");
    tui.type_str(&format!("{}/", alpha.parent().unwrap().display()));
    tui.wait_for_text("alpha-proj/");
    tui.wait_for_text("beta-proj/");
    tui.send(&[0x1b]); // Esc
    tui.wait_for_gone("Add project");

    // ---- second project typed the plain way ----
    add_project(&mut tui, &beta, "beta-proj");
    // First project stays selected; its rows render reversed in the focused panel.
    tui.wait_for_selected("alpha-proj");

    // ---- Tab cycles focus across all four panes and back ----
    tui.send(b"\t");
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.send(b"\t");
    tui.wait_for_text(FOOTER_SESSIONS);
    tui.send(b"\t");
    // Terminal pane focused with nothing attached: no panel footer, no lock.
    tui.wait_for_gone(FOOTER_SESSIONS);
    tui.send(b"\t");
    tui.wait_for_text(FOOTER_PROJECTS); // wrapped around

    // ---- Enter drills from Projects into Worktrees ----
    tui.send(b"\r");
    tui.wait_for_text(FOOTER_WORKTREES);

    // ---- create two worktrees on alpha-proj ----
    create_worktree(&mut tui, "feat-a");
    create_worktree(&mut tui, "feat-b");

    // The worktree dirs exist on disk, as siblings of the repo.
    let wt_root = alpha.parent().unwrap().join("alpha-proj-worktrees");
    assert!(wt_root.join("feat-a").exists(), "feat-a worktree on disk");
    assert!(wt_root.join("feat-b").exists(), "feat-b worktree on disk");

    // ---- a fresh worktree is auto-selected: feat-b was created last ----
    tui.wait_for_selected("feat-b");

    // ---- j/k still walks the selection: feat-b → feat-a → main → feat-a ----
    tui.send(b"k");
    tui.wait_for_selected("feat-a");
    tui.send(b"k");
    tui.wait_for_selected("main ⌂ root");
    tui.send(b"j");
    tui.wait_for_selected("feat-a");

    // ---- Enter shows the sessions (agents) panel for feat-a ----
    tui.send(b"\r");
    tui.wait_for_text(FOOTER_SESSIONS);

    // ---- create an agent: kind picker → name prompt, auto-attaches ----
    tui.send(b"n");
    tui.wait_for_text("New session"); // Claude/Codex/Cursor/Terminal picker
    tui.send(b"\r"); // pick the default (Claude)
    tui.wait_for_gone("New session");
    tui.wait_for_text("New agent");
    tui.send(b"\r"); // empty input falls back to "agent-1"
    tui.wait_for_gone("New agent");
    tui.wait_for_text("agent-1"); // now provably the sessions-panel row
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED); // auto-attach locks input

    // ---- Ctrl+q (raw byte 0x11, what every emulator sends) escapes back ----
    tui.send(&[0x11]);
    tui.wait_for_text(FOOTER_SESSIONS);

    // ---- Tab merely focuses the live pane (no lock); Enter locks it ----
    tui.send(b"\t");
    tui.wait_for_text(FOOTER_TERMINAL_FOCUSED);
    tui.send(b"\r");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
    tui.send(&[0x1d]); // Ctrl+] fallback (legacy byte, what Terminal.app sends)
    tui.wait_for_text(FOOTER_SESSIONS);

    // ---- Shift+T: a shell terminal in the worktree dir, auto-attached ----
    tui.send(b"T");
    tui.wait_for_text("TERMINALS");
    tui.wait_for_text("term-1");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
    tui.send(&[0x11]); // Ctrl+q back to panels
    tui.wait_for_text(FOOTER_SESSIONS);

    // ---- sessions are per-worktree: feat-b has no agent-1 ----
    tui.send(b"\x1b[D"); // ← back to Worktrees (feat-a still selected)
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.send(b"j"); // feat-b
    tui.wait_for_selected("feat-b");
    tui.wait_for_sessions_row_gone("agent-1");
    tui.send(b"k"); // back to feat-a
    tui.wait_for_selected("feat-a");
    tui.wait_for_text("agent-1");

    // ---- toggling projects swaps the whole worktree panel ----
    tui.send(b"\x1b[D"); // ← focus Projects
    tui.wait_for_text(FOOTER_PROJECTS);
    tui.send(b"j"); // select beta-proj
    tui.wait_for_selected("beta-proj");
    tui.wait_for_gone("feat-a"); // beta has only its main checkout
    tui.wait_for_text("main ⌂ root");
    tui.wait_for_sessions_row_gone("agent-1");

    // ---- the root row tracks live branch switches, no restart needed ----
    repo_git(&beta, &["checkout", "-b", "hotfix"]);
    tui.wait_for_text("hotfix ⌂ root");
    tui.send(b"k"); // back to alpha-proj
    tui.wait_for_selected("alpha-proj");
    tui.wait_for_text("feat-a");
    tui.wait_for_text("feat-b");

    // Switching back restores the remembered context: feat-a is the
    // selected worktree again and its agent is back without re-drilling.
    tui.wait_for_text("agent-1");
    tui.send(b"\r"); // Projects → Worktrees
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.wait_for_selected("feat-a");

    // ---- clean quit ----
    tui.send(b"q");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tui.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => panic!(
                "TUI did not exit after q\n--- screen ---\n{}",
                tui.screen_text()
            ),
        }
    }
}

#[test]
fn tui_help_modal_grouped_keymap() {
    let mut tui = TuiHarness::spawn();
    tui.wait_for_text("create your first project");

    // The grouped two-column keymap: every section header on screen at
    // once, including the note hotkey (the old single list clipped its
    // tail on short terminals).
    tui.send(b"?");
    tui.wait_for_text("NAVIGATE & SEARCH");
    tui.wait_for_text("PROJECTS");
    tui.wait_for_text("WORKTREES");
    tui.wait_for_text("SESSIONS");
    tui.wait_for_text("TERMINAL & MOUSE");
    tui.wait_for_text("GENERAL");
    tui.wait_for_text("notes for the worktree");
    tui.wait_for_text("project-level notes");

    tui.send(&[0x1b]); // Esc closes
    tui.wait_for_gone("NAVIGATE & SEARCH");
}

#[test]
fn tui_note_modal_crud_and_badge() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("note-proj");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "note-proj");
    // The root worktree row must exist before e has a list to open.
    tui.wait_for_text("⌂ root");

    // ---- Projects focus: t opens the PROJECT's own list (no branch in
    // the title — the trailing space keeps it from matching ".../main") ----
    tui.send(b"e");
    tui.wait_for_text("Notes — note-proj ");
    tui.wait_for_text("no notes yet");
    tui.send(b"e"); // start the add input
    tui.type_str("project level plan");
    tui.send(b"\r");
    tui.wait_for_text("☐ project level plan");
    tui.send(b" "); // done — the project badge reads ✓1
    tui.wait_for_text("✓ project level plan");
    tui.send(&[0x1b]); // Esc closes
    tui.wait_for_gone("Notes — note-proj ");
    tui.wait_for_text("✓1"); // project-row badge

    // ---- Worktrees focus: t opens the WORKTREE's list — a separate,
    // still-empty set of notes ----
    tui.send(b"\r"); // Projects → Worktrees
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.send(b"e");
    tui.wait_for_text("Notes — note-proj/main");
    tui.wait_for_text("no notes yet");

    // ---- create ----
    tui.send(b"e"); // start the add input
    tui.type_str("ship the feature");
    tui.send(b"\r");
    tui.wait_for_text("☐ ship the feature");
    tui.wait_for_text("(1 open)");
    tui.send(b"a"); // a second note, via the alternate add key
    tui.type_str("write docs");
    tui.send(b"\r");
    tui.wait_for_text("☐ write docs");
    tui.wait_for_text("(2 open)");

    // ---- update: the cursor sits on the just-created note ----
    tui.send(b"\r"); // edit "write docs" (prefilled)
    tui.type_str(" tomorrow");
    tui.send(b"\r");
    tui.wait_for_text("☐ write docs tomorrow");

    // ---- toggle done ----
    tui.send(b" ");
    tui.wait_for_text("✓ write docs tomorrow");
    tui.wait_for_text("(1 open)");

    // ---- the worktree row badge shows the open count ----
    tui.send(&[0x1b]); // Esc closes the modal
    tui.wait_for_gone("Notes — note-proj/main");
    tui.wait_for_text("✎1");

    // ---- delete: k up to the open note, d removes it ----
    tui.send(b"e");
    tui.wait_for_text("Notes — note-proj/main");
    tui.send(b"k");
    tui.wait_for_selected("☐ ship the feature");
    tui.send(b"d");
    tui.wait_for_gone("ship the feature");
    tui.wait_for_text("(all 1 done)");

    // ---- deleting the last worktree note empties only THIS list ----
    tui.send(b"d");
    tui.wait_for_text("no notes yet");
    tui.send(&[0x1b]);
    tui.wait_for_gone("Notes — note-proj/main");

    // ---- the project's own note survived untouched ----
    tui.send(b"\x1b[D"); // ← back to Projects focus
    tui.wait_for_text(FOOTER_PROJECTS);
    tui.send(b"e");
    tui.wait_for_text("Notes — note-proj ");
    tui.wait_for_text("✓ project level plan");
    tui.send(&[0x1b]);
    tui.wait_for_gone("Notes — note-proj ");
}

#[test]
fn tui_git_diff_modal() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("diff-proj");

    tui.wait_for_text("create your first project");
    add_project(&mut tui, &repo, "diff-proj");
    // The root worktree row must exist before g has anything to diff.
    tui.wait_for_text("⌂ root");

    // Dirty the checkout: one tracked modification, one untracked file.
    std::fs::write(repo.join(".keep"), "tracked change\n").unwrap();
    std::fs::write(repo.join("hello.txt"), "hello world\n").unwrap();

    // The worktree panel's bottom badge picks the changes up on its own
    // poll — no keypress in between.
    tui.wait_for_text("+2 files");

    // ---- open the modal; the selected file's diff renders ----
    tui.send(b"g");
    tui.wait_for_text("Files (2)");
    // Status is path-ordered, so .keep (modified) is selected first.
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");

    // ---- Ctrl+r marks .keep reviewed: it sinks below hello.txt and the
    // selection auto-advances to the next file, loading its diff ----
    tui.send(&[0x12]);
    tui.wait_for_text("· 1✓"); // files-panel title counts the mark
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+hello world");

    // ---- Down reaches the reviewed zone; Ctrl+r unmarks .keep, which
    // pops back to the top of the list and stays selected ----
    tui.send(b"\x1b[B"); // Down
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");
    tui.send(&[0x12]);
    tui.wait_for_gone("· 1✓");

    // ---- arrow to the untracked file ----
    tui.send(b"\x1b[B"); // Down
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+hello world");

    // ---- type-to-filter narrows the list and reselects the top match ----
    tui.type_str("kee");
    tui.wait_for_text("Files (1/2)");
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+tracked change");
    tui.send(&[0x1b]); // first Esc clears the filter, not the modal
    tui.wait_for_text("Files (2)");

    // ---- the modal blocks other interaction ----
    // n would open "Add project" from the Projects panel; inside the modal it
    // feeds the filter instead (verified after close — stale-frame convention).
    tui.send(b"n");
    tui.wait_for_text("no matches");
    tui.send(&[0x1b]); // Esc clears the filter…
    tui.wait_for_text("Files (2)"); // (also keeps the two Escs from coalescing)
    tui.send(&[0x1b]); // …and the second closes the modal
    tui.wait_for_gone("Files (2)");
    tui.wait_for_text(FOOTER_PROJECTS);
    assert!(
        !tui.screen_text().contains("Add project"),
        "modal swallowed n\n--- screen ---\n{}",
        tui.screen_text()
    );

    // ---- clean tree flashes instead of opening ----
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-m", "wip"]);
    // The commit empties the badge on the next poll.
    tui.wait_for_gone("+2 files");
    tui.send(b"g");
    tui.wait_for_text("no changes in main");
}
