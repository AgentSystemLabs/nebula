//! Panic capture to a log file. The TUI owns the terminal (panic output to
//! stderr is lost with the alternate screen) and the daemon runs with stderr
//! on /dev/null, so a hook that appends to the process's log file is the only
//! way a crash leaves a trace.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Install a panic hook that appends the panic message and a backtrace to
/// `path`, then delegates to the previously installed hook. Fires for panics
/// on any thread, including tokio worker threads.
pub fn install_panic_hook(path: PathBuf) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // force_capture: backtrace regardless of RUST_BACKTRACE.
        let backtrace = std::backtrace::Backtrace::force_capture();
        append(&path, &format!("PANIC {info}\nbacktrace:\n{backtrace}"));
        prev(info);
    }));
}

/// Best-effort timestamped append; a crash logger must never itself panic.
pub fn append(path: &Path, msg: &str) {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{} {msg}", timestamp())
    };
    let _ = write();
}

/// UTC timestamp matching the tracing-subscriber log line format closely
/// enough to sort/grep alongside it, without pulling in a time crate.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let tod = secs % 86400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Gregorian date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch_and_leap() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn panic_hook_captures_thread_panic() {
        let dir = std::env::temp_dir().join(format!("nebula-panichook-{}", std::process::id()));
        let path = dir.join("crash.log");
        install_panic_hook(path.clone());
        let _ = std::thread::spawn(|| panic!("crashlog-hook-test")).join();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("PANIC") && text.contains("crashlog-hook-test"), "got: {text}");
        assert!(text.contains("backtrace:"), "got: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_creates_parent_and_writes() {
        let dir = std::env::temp_dir().join(format!("nebula-crashlog-{}", std::process::id()));
        let path = dir.join("nested").join("crash.log");
        append(&path, "PANIC test");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("PANIC test"), "got: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
