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
const FOOTER_PROJECTS: &str = "n: add";
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
        // runtime dir short and per-process.
        let pid = std::process::id();
        let runtime_dir = PathBuf::from(format!("/tmp/nebtui-rt-{pid}"));
        let data_dir = PathBuf::from(format!("/tmp/nebtui-data-{pid}"));
        let _ = std::fs::remove_dir_all(&runtime_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        let repos = tempfile::tempdir().unwrap();

        let pty = native_pty_system()
            .openpty(PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_nebula"));
        cmd.env("NEBULA_RUNTIME_DIR", &runtime_dir);
        cmd.env("NEBULA_DATA_DIR", &data_dir);
        cmd.env("NEBULA_AGENT_CMD", "/bin/sh"); // stand-in for claude
        cmd.env("NEBULA_LOG", "debug");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("TERM", "xterm-256color");
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

        Self { writer, parser, child, runtime_dir, data_dir, _repos: repos }
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
        self.wait_for(&format!("text {needle:?}"), |s| screen_to_text(s).contains(needle));
    }

    fn wait_for_gone(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?} to disappear"), |s| {
            !screen_to_text(s).contains(needle)
        });
    }

    /// Wait until the row containing `needle` renders with the REVERSED
    /// attribute — i.e. it is the selected row of a focused panel.
    fn wait_for_selected(&self, needle: &str) {
        self.wait_for(&format!("row {needle:?} selected (reversed)"), |s| {
            row_is_reversed(s, needle)
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
            .arg("kill-server")
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
                    let contents: String = cell.contents().into();
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
/// (mirrors PROJECTS_W/WORKTREES_W/SESSIONS_W in nebula-tui/src/ui.rs).
fn sessions_panel_contains(screen: &vt100::Screen, needle: &str) -> bool {
    const SESSIONS_X: u16 = 20 + 22;
    const SESSIONS_W: u16 = 26;
    let (rows, cols) = screen.size();
    let right = SESSIONS_X.saturating_add(SESSIONS_W).min(cols);
    for row in 0..rows {
        let mut line = String::new();
        for col in SESSIONS_X..right {
            let contents: String =
                screen.cell(row, col).map(|c| c.contents().into()).unwrap_or_default();
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

fn row_is_reversed(screen: &vt100::Screen, needle: &str) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        let mut line = String::new();
        for col in 0..cols {
            let contents: String =
                screen.cell(row, col).map(|c| c.contents().into()).unwrap_or_default();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&contents);
            }
        }
        if line.contains(needle) {
            let reversed =
                (0..cols).any(|col| screen.cell(row, col).map(|c| c.inverse()).unwrap_or(false));
            if reversed {
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

fn create_worktree(tui: &mut TuiHarness, branch: &str) {
    tui.send(b"n");
    tui.wait_for_text("New worktree");
    tui.type_str(branch);
    tui.send(b"\r");
    tui.wait_for_gone("New worktree");
    tui.wait_for_text(branch);
}

#[test]
fn tui_projects_worktrees_agents_navigation() {
    let mut tui = TuiHarness::spawn();
    let alpha = tui.make_repo("alpha-proj");
    let beta = tui.make_repo("beta-proj");

    // ---- boot: empty state, Projects focused ----
    tui.wait_for_text("No projects yet");
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
    tui.wait_for_text("main (main)"); // main checkout appears as a worktree row

    // Ambiguous completion lists candidates: "…/T/.tmpX/" + Tab with both
    // repos present shows them side by side, then Esc cancels.
    tui.send(b"n");
    tui.wait_for_text("Add project");
    tui.type_str(&format!("{}/", alpha.parent().unwrap().display()));
    tui.send(b"\t");
    tui.wait_for_text("alpha-proj/  beta-proj/");
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

    // ---- j/k moves the worktree selection: main → feat-a → feat-b → feat-a ----
    tui.wait_for_selected("main (main)");
    tui.send(b"j");
    tui.wait_for_selected("feat-a");
    tui.send(b"j");
    tui.wait_for_selected("feat-b");
    tui.send(b"k");
    tui.wait_for_selected("feat-a");

    // ---- Enter shows the sessions (agents) panel for feat-a ----
    tui.send(b"\r");
    tui.wait_for_text(FOOTER_SESSIONS);
    tui.wait_for_text("AGENTS");
    tui.wait_for_text("TERMINALS");

    // ---- create an agent: default name accepted, auto-attaches ----
    tui.send(b"n");
    tui.wait_for_text("New agent");
    tui.send(b"\r"); // accept the pre-filled "agent-1"
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

    // ---- sessions are per-worktree: feat-b has no agent-1 ----
    tui.send(b"h"); // back to Worktrees (feat-a still selected)
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.send(b"j"); // feat-b
    tui.wait_for_selected("feat-b");
    tui.wait_for_sessions_row_gone("agent-1");
    tui.send(b"k"); // back to feat-a
    tui.wait_for_selected("feat-a");
    tui.wait_for_text("agent-1");

    // ---- toggling projects swaps the whole worktree panel ----
    tui.send(b"h"); // focus Projects
    tui.wait_for_text(FOOTER_PROJECTS);
    tui.send(b"j"); // select beta-proj
    tui.wait_for_selected("beta-proj");
    tui.wait_for_gone("feat-a"); // beta has only its main checkout
    tui.wait_for_text("main (main)");
    tui.wait_for_sessions_row_gone("agent-1");
    tui.send(b"k"); // back to alpha-proj
    tui.wait_for_selected("alpha-proj");
    tui.wait_for_text("feat-a");
    tui.wait_for_text("feat-b");

    // Switching projects resets the worktree selection to main (by design),
    // so drill back into feat-a and find the agent again.
    tui.send(b"\r"); // Projects → Worktrees
    tui.wait_for_text(FOOTER_WORKTREES);
    tui.wait_for_selected("main (main)");
    tui.send(b"j"); // feat-a
    tui.wait_for_selected("feat-a");
    tui.wait_for_text("agent-1");

    // ---- clean quit ----
    tui.send(b"q");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tui.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => panic!("TUI did not exit after q\n--- screen ---\n{}", tui.screen_text()),
        }
    }
}
