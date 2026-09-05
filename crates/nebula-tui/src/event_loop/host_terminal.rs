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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::ThreadId;
use std::time::Duration;

pub(super) type HostTerminal = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// How often the runtime modes are re-asked while the loop runs. Every
/// enable is idempotent and the whole burst is a few dozen bytes, so the
/// beat is set by how long a dead mouse may stay dead, not by cost.
pub(super) const MODE_REASSERT: Duration = Duration::from_secs(2);

/// The kitty keyboard flags pushed on the host: without them Cmd-combos
/// never reach us and Option/Esc combos arrive ambiguous.
const KITTY_FLAGS: KeyboardEnhancementFlags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;

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
    let _ = disable_raw_mode();
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
        // user's shell in disambiguate mode.
        write!(w, "\x1b[={};1u", KITTY_FLAGS.bits())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
