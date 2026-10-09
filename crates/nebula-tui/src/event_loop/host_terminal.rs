//! The host terminal — the one nebula itself is drawn in — and the modes it
//! is asked for at startup: the alternate screen, mouse reporting, bracketed
//! paste, focus reports and the kitty keyboard flags. `setup_terminal` asks
//! for them and `restore_terminal` hands the terminal back; the rest of this
//! module keeps them true while the loop runs.
//!
//! Two things take them away mid-session. A panic on a worker thread (a
//! `tokio::spawn`ed `gh` lookup, the vim reader thread) unwinds only that
//! thread — the loop survives — so a panic hook that restores the terminal
//! for *every* thread leaves a live UI with no mouse, no raw mode and the
//! primary screen under it. And the host can forget the modes on its own:
//! iTerm2's Session ▸ Reset (⌘R) or a stray RIS clears mouse reporting and
//! the alternate screen without telling the application, after which every
//! click goes to the terminal and the wheel scrolls its scrollback instead
//! of the pane. The hook here restores only for the thread that owns the
//! terminal, and the modes are re-asked on a slow beat and on every resize.

use anyhow::Result;
use crossterm::event::KeyboardEnhancementFlags;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{BufWriter, Stdout, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::thread::ThreadId;
use std::time::Duration;

pub(super) type HostTerminal = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// How often the runtime modes are re-asked while the loop runs. Every
/// enable is idempotent and the whole burst is a few dozen bytes, so the
/// beat is set by how long a dead mouse may stay dead, not by cost.
pub(super) const MODE_REASSERT: Duration = Duration::from_secs(2);

/// The kitty keyboard flags pushed on the host: without them Cmd-combos
/// never reach us and Option/Esc combos arrive ambiguous. The flags the
/// host RESTS on — `HOLD_FLAGS` is the one moment it is asked for more.
const KITTY_FLAGS: KeyboardEnhancementFlags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;

/// The flags while a RELEASE WATCH is on (release_watch.rs): every key as
/// an escape code, so a held text key's repeats and release are reported
/// and marked, plus the alternate keys so a shifted key still reads as
/// the character it types (`CSI 59:58;2u` is `:`, not `;` with shift).
/// Never the resting flags: under them the host sends key codes in place
/// of text, and composed text — a dead key's `é`, a non-Latin layout —
/// is lost.
pub(super) const HOLD_FLAGS: KeyboardEnhancementFlags = KITTY_FLAGS
    .union(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    .union(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS)
    .union(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES);

/// The flags the host holds right now — `KITTY_FLAGS`, or `HOLD_FLAGS`
/// while a RELEASE WATCH is on — for the beat that re-asks them.
static HOST_FLAGS: AtomicU8 = AtomicU8::new(KITTY_FLAGS.bits());

/// Re-entering the alternate screen: `?1047h`, not `?1049h`. Both are no-ops
/// on a terminal already showing it, but 1049 also saves the cursor, and the
/// `?1049l` at exit would restore that alternate-screen position onto the
/// user's shell instead of the one saved at startup.
const ALT_SCREEN_REENTER: &[u8] = b"\x1b[?1047h";

/// Whether we pushed kitty keyboard flags on the outer terminal (so restore —
/// including the panic hook — knows to pop them).
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);

/// Panics on threads other than the one that owns the terminal, counted by
/// the panic hook and drained by the loop through `take_worker_panic`.
static WORKER_PANICS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn setup_terminal() -> Result<HostTerminal> {
    use crossterm::{execute, terminal::*};
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        // Focus reports (mode 1004): coming back from the browser is the
        // moment a pull request was most likely just closed there.
        crossterm::event::EnableFocusChange,
    )?;
    // Kitty keyboard protocol on the outer terminal. Probe first —
    // Terminal.app and friends don't speak it (must happen before the
    // EventStream exists; the probe reads stdin).
    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        use crossterm::event::PushKeyboardEnhancementFlags;
        execute!(stdout, PushKeyboardEnhancementFlags(KITTY_FLAGS))?;
        KITTY_PUSHED.store(true, Ordering::Relaxed);
    }
    // The thread running the loop is the one whose panic takes the process
    // down, and the only one whose panic should take the terminal with it.
    install_panic_hook(std::thread::current().id(), restore_terminal);
    // Buffered so a full-frame redraw reaches the terminal in a few large
    // writes instead of one syscall per line (Stdout is line-buffered).
    let writer = BufWriter::with_capacity(64 * 1024, std::io::stdout());
    Ok(Terminal::new(CrosstermBackend::new(writer))?)
}

pub fn restore_terminal() {
    use crossterm::{execute, terminal::*};
    // Pop while still on the alternate screen — kitty keeps a keyboard-flag
    // stack per screen, so the pop must land on the screen that pushed.
    if KITTY_PUSHED.swap(false, Ordering::Relaxed) {
        let _ = execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    let _ = execute!(
        std::io::stdout(),
        // Hand back the default pointer in case we left it col-resize
        // (OSC 22; terminals without pointer-shape support drop it).
        crossterm::style::Print("\x1b]22;default\x1b\\"),
        crossterm::event::DisableFocusChange,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen,
    );
    // Nebula never changes its own cwd. Hand that directory back to the
    // shell instead of leaving the last previewed checkout on the host.
    if !nebula_core::host::is_remote_session() {
        if let Ok(cwd) = std::env::current_dir() {
            let mut stdout = std::io::stdout().lock();
            let _ = write_working_directory(&mut stdout, &cwd);
            let _ = stdout.flush();
        }
    }
    let _ = disable_raw_mode();
}

/// Publish the visible session's checkout through OSC 7, so the outer
/// terminal can resolve relative links even though child OSCs are consumed
/// by vt100. An unchanged directory produces no output.
pub(super) fn report_working_directory(
    app: &crate::app::App,
    sent: &mut Option<PathBuf>,
    w: &mut impl Write,
) -> std::io::Result<()> {
    // ponytail: local checkout roots only; child-shell cd and remote host
    // identity need separate tracking, never advertise SSH paths as local.
    if app.chrome.is_remote {
        return Ok(());
    }
    let cwd = super::attached_worktree_root(app).or_else(|| std::env::current_dir().ok());
    if cwd != *sent {
        if let Some(path) = &cwd {
            write_working_directory(w, path)?;
            w.flush()?;
        }
        *sent = cwd;
    }
    Ok(())
}

fn write_working_directory(w: &mut impl Write, path: &Path) -> std::io::Result<()> {
    if !path.is_absolute() {
        return Ok(());
    }
    w.write_all(b"\x1b]7;file://localhost")?;
    // Encode raw path bytes: spaces, URL delimiters and terminal controls
    // must stay filename data, and non-UTF-8 names must round-trip too.
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            w.write_all(&[byte])?;
        } else {
            write!(w, "%{byte:02X}")?;
        }
    }
    w.write_all(b"\x1b\\")
}

/// Wrap whatever panic hook is installed (the crash log's, which chains to
/// the default) so the terminal is restored before the panic message prints
/// — but only when the panic is on `owner`, the thread whose unwinding ends
/// the process. Any other thread's panic is caught by tokio or dies with
/// its thread while the loop goes on; it is counted for the loop to notice
/// and left to the crash log to describe.
fn install_panic_hook(owner: ThreadId, on_owner_panic: fn()) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == owner {
            on_owner_panic();
        } else {
            WORKER_PANICS.fetch_add(1, Ordering::Relaxed);
        }
        prev(info);
    }));
}

/// Whether a worker thread has panicked since the last call. The default
/// hook printed its message onto the alternate screen, so the caller owes a
/// repaint — and the user a word about where the details went.
pub(super) fn take_worker_panic() -> bool {
    WORKER_PANICS.swap(0, Ordering::Relaxed) > 0
}

/// Re-ask the host for the runtime modes it may have dropped: mouse
/// reporting, bracketed paste, focus reports and — when the startup probe
/// found a kitty-protocol terminal — the keyboard flags.
pub(super) fn reassert_modes(w: &mut impl Write) -> std::io::Result<()> {
    write_modes(w, KITTY_PUSHED.load(Ordering::Relaxed))?;
    w.flush()
}

/// A host resize: the one moment a terminal that reset itself will be
/// repainted from scratch anyway, so also re-enter the alternate screen
/// and re-ask the modes, then repaint so ratatui draws every cell rather
/// than the diff against a frame the host no longer shows.
pub(super) fn on_host_resize(terminal: &mut HostTerminal) -> Result<()> {
    let backend = terminal.backend_mut();
    write_resize_recovery(backend, KITTY_PUSHED.load(Ordering::Relaxed))?;
    backend.flush()?;
    repaint(terminal)
}

/// Clear the screen and forget the last frame, so the next draw emits
/// every cell. Not `Terminal::clear`: that opens with a cursor-position
/// query (`CSI 6n`) and blocks up to two seconds for the reply, and a
/// host that never answers — a pty under test, a recorder — would end the
/// loop with "the cursor position could not be read". `resize` to the
/// size we already have clears and resets the diff buffer without asking.
pub(super) fn repaint(terminal: &mut HostTerminal) -> Result<()> {
    let area = terminal.size()?.into();
    terminal.resize(area)?;
    Ok(())
}

fn write_resize_recovery(w: &mut impl Write, kitty: bool) -> std::io::Result<()> {
    w.write_all(ALT_SCREEN_REENTER)?;
    write_modes(w, kitty)
}

fn write_modes(w: &mut impl Write, kitty: bool) -> std::io::Result<()> {
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange, EnableMouseCapture};
    crossterm::queue!(
        w,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    if kitty {
        // The protocol's *set* form (CSI = flags ; 1 u): it rewrites the
        // entry `setup_terminal` pushed. A second push would leave one
        // entry on the host's stack after the single pop at exit, and the
        // user's shell in disambiguate mode. The flags as they stand: a
        // RELEASE WATCH in progress keeps its own.
        write_flags(w, HOST_FLAGS.load(Ordering::Relaxed))?;
    }
    Ok(())
}

/// The set form of the kitty flags (see `write_modes`).
fn write_flags(w: &mut impl Write, flags: u8) -> std::io::Result<()> {
    write!(w, "\x1b[={flags};1u")
}

/// Flip the host onto `HOLD_FLAGS` (`on`) for a RELEASE WATCH, or back to
/// the resting flags — at once, ahead of the held key's first repeat. A
/// host that never took the flags is written nothing: it has no repeats
/// to mark, and the watch there is only as good as legacy presses.
pub(super) fn watch_held_key(w: &mut impl Write, on: bool) -> std::io::Result<()> {
    if !KITTY_PUSHED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let flags = if on { HOLD_FLAGS } else { KITTY_FLAGS };
    HOST_FLAGS.store(flags.bits(), Ordering::Relaxed);
    write_flags(w, flags.bits())?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AttachedTerm};
    use crate::event_loop::tests::{seed_feat_worktree, seed_tree};
    use nebula_core::{AgentId, SessionRef, TerminalId, TerminalTab, WorktreeId};

    #[test]
    fn working_directory_follows_the_attached_agent_not_the_sidebar() {
        let mut app = App::new();
        app.chrome.is_remote = false;
        seed_tree(&mut app);
        seed_feat_worktree(&mut app, "w2", "links");
        app.pane.term = Some(AttachedTerm::new(
            SessionRef::Agent(AgentId("a1".into())),
            80,
            24,
        ));
        let mut sent = None;
        let mut out = Vec::new();
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert_eq!(text(&out), "\x1b]7;file://localhost/tmp/demo\x1b\\");
        out.clear();
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert!(out.is_empty(), "unchanged cwd is not repeated");

        // An agent moved checkout while the sidebar still selects main.
        app.tree.agents[0].worktree_id = WorktreeId("w2".into());
        assert_eq!(
            app.selected_worktree().unwrap().path,
            Path::new("/tmp/demo")
        );
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert_eq!(
            text(&out),
            "\x1b]7;file://localhost/tmp/demo-worktrees/links\x1b\\"
        );

        out.clear();
        app.pane.term = None;
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert_eq!(text(&out), "\x1b]7;file://localhost/tmp/demo\x1b\\");
    }

    #[test]
    fn working_directory_follows_an_attached_shell_and_leaves_ssh_alone() {
        let mut app = App::new();
        app.chrome.is_remote = false;
        seed_tree(&mut app);
        seed_feat_worktree(&mut app, "w2", "shell");
        let id = TerminalId("t1".into());
        app.tree.terminals.push(TerminalTab {
            id: id.clone(),
            worktree_id: WorktreeId("w2".into()),
            name: "shell".into(),
            sort_order: 0,
            alive: true,
            run_command: None,
        });
        app.pane.term = Some(AttachedTerm::new(SessionRef::Terminal(id), 80, 24));
        let mut sent = None;
        let mut out = Vec::new();
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert_eq!(
            text(&out),
            "\x1b]7;file://localhost/tmp/demo-worktrees/shell\x1b\\"
        );

        app.chrome.is_remote = true;
        sent = None;
        out.clear();
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert!(
            out.is_empty(),
            "remote paths must not become local file URLs"
        );
        assert!(sent.is_none());
    }

    #[test]
    fn working_directory_encodes_url_delimiters_controls_and_non_utf8_bytes() {
        let path = Path::new(std::ffi::OsStr::from_bytes(
            b"/tmp/a b#?%/caf\xc3\xa9/\xff\x07\x1b\\",
        ));
        let mut out = Vec::new();
        write_working_directory(&mut out, path).unwrap();
        assert_eq!(
            text(&out),
            "\x1b]7;file://localhost/tmp/a%20b%23%3F%25/caf%C3%A9/%FF%07%1B%5C\x1b\\"
        );
        out.clear();
        write_working_directory(&mut out, Path::new("relative/path")).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn working_directory_retries_a_failed_write() {
        let mut app = App::new();
        app.chrome.is_remote = false;
        seed_tree(&mut app);
        let mut sent = None;
        let mut full = &mut [][..];
        assert!(report_working_directory(&app, &mut sent, &mut full).is_err());
        assert!(sent.is_none(), "only a successful write counts as reported");
        let mut out = Vec::new();
        report_working_directory(&app, &mut sent, &mut out).unwrap();
        assert_eq!(text(&out), "\x1b]7;file://localhost/tmp/demo\x1b\\");
    }

    #[test]
    fn a_worker_thread_panic_is_counted_and_leaves_the_terminal_alone() {
        static OWNER_PANICS: AtomicUsize = AtomicUsize::new(0);
        fn note_owner_panic() {
            OWNER_PANICS.fetch_add(1, Ordering::Relaxed);
        }
        install_panic_hook(std::thread::current().id(), note_owner_panic);
        let _ = std::thread::spawn(|| panic!("host-terminal-worker-panic")).join();
        assert!(
            take_worker_panic(),
            "the worker's panic is counted for the loop"
        );
        assert!(!take_worker_panic(), "and drained by the read");
        assert_eq!(
            OWNER_PANICS.load(Ordering::Relaxed),
            0,
            "a worker thread's panic never restores the terminal"
        );

        // The owner's own panic is the fatal one: that is when the
        // terminal is handed back.
        let _ = std::panic::catch_unwind(|| panic!("host-terminal-owner-panic"));
        assert_eq!(OWNER_PANICS.load(Ordering::Relaxed), 1);
        assert!(!take_worker_panic(), "and it is not a worker panic");
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn reasserting_asks_for_every_runtime_mode_again() {
        let mut out = Vec::new();
        write_modes(&mut out, false).unwrap();
        let out = text(&out);
        for mode in ["?1000h", "?1002h", "?1003h", "?1006h", "?2004h", "?1004h"] {
            assert!(out.contains(mode), "{mode} missing from {out:?}");
        }
        assert!(
            !out.contains("?1049h"),
            "never re-enters the screen: {out:?}"
        );
        assert!(
            !out.contains('u'),
            "no kitty flags without the probe: {out:?}"
        );
    }

    #[test]
    fn kitty_flags_are_set_in_place_never_pushed_again() {
        let mut out = Vec::new();
        write_modes(&mut out, true).unwrap();
        let out = text(&out);
        assert!(out.ends_with("\x1b[=1;1u"), "the set form: {out:?}");
        assert!(
            !out.contains("\x1b[>"),
            "a push would leak past the exit pop: {out:?}"
        );
    }

    /// The RELEASE WATCH's flags go the same way — set in place, never
    /// pushed — and ask for the event types, the alternate keys and every
    /// key as an escape code on top of the resting disambiguation. A host
    /// that never took the flags is written nothing.
    #[test]
    fn the_hold_flags_are_set_in_place_too() {
        let mut out = Vec::new();
        write_flags(&mut out, HOLD_FLAGS.bits()).unwrap();
        let out = text(&out);
        assert_eq!(out, "\x1b[=15;1u", "1 | 2 | 4 | 8");
        let mut none = Vec::new();
        watch_held_key(&mut none, true).unwrap();
        assert!(none.is_empty(), "{:?}", text(&none));
    }

    #[test]
    fn a_resize_re_enters_the_alternate_screen_without_saving_the_cursor() {
        let mut out = Vec::new();
        write_resize_recovery(&mut out, false).unwrap();
        let out = text(&out);
        assert!(out.starts_with("\x1b[?1047h"), "{out:?}");
        assert!(
            !out.contains("?1049"),
            "1049 would clobber the saved cursor: {out:?}"
        );
        assert!(out.contains("?1000h"), "and the modes ride along: {out:?}");
    }

    /// ratatui's whole-terminal clear method opens with a cursor-position query (`CSI 6n`) and
    /// crossterm gives up on the reply after 2 s with an error the loop turns into a fatal exit;
    /// `repaint` clears through `Terminal::resize`, which asks nothing. So nothing in the loop may
    /// call it — this reads the loop's own source, because the trap is a call site, not a
    /// behaviour a TestBackend could show. (The needles are assembled so this test's own text
    /// never matches them.)
    #[test]
    fn the_loop_never_calls_terminal_clear() {
        let needles = [
            concat!("terminal.", "clear("),
            concat!("Terminal::", "clear("),
        ];
        let sources = [
            ("event_loop.rs", include_str!("../event_loop.rs")),
            ("event_loop/focus_walk.rs", include_str!("focus_walk.rs")),
            (
                "event_loop/host_terminal.rs",
                include_str!("host_terminal.rs"),
            ),
        ];
        for (name, src) in sources {
            for needle in needles {
                assert!(
                    !src.contains(needle),
                    "{name} calls Terminal::clear — use host_terminal::repaint (Terminal::resize)"
                );
            }
        }
    }
}
