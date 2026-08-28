use crate::env;
use std::path::{Path, PathBuf};

/// Where the runtime dir lands when neither override nor `XDG_RUNTIME_DIR`
/// is set. World-writable, so the per-uid subdir is what carries mode 0700.
const FALLBACK_RUNTIME_ROOT: &str = "/tmp";

/// Runtime dir holding the socket + pidfile. Mode 0700 — this is the auth
/// boundary, same model as tmux. `NEBULA_RUNTIME_DIR` overrides (tests,
/// parallel instances).
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::non_empty(env::RUNTIME_DIR) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env::non_empty("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("nebula");
    }
    let uid = libc_geteuid();
    Path::new(FALLBACK_RUNTIME_ROOT).join(format!("nebula-{uid}"))
}

// Avoid a libc dependency in this dep-light crate for one call.
fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn pidfile_path() -> PathBuf {
    runtime_dir().join("daemon.pid")
}

/// Fingerprint of the binary the running daemon was launched from, written
/// by the daemon at startup. Installers compare it against the binary they
/// just installed to tell an up-to-date daemon from a stale one.
pub fn buildstamp_path() -> PathBuf {
    runtime_dir().join("daemon.build")
}

/// The platform's per-user dirs for this app (`~/Library/Application
/// Support/dev.nebula.nebula` on macOS, `~/.local/share/nebula` on Linux).
fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("dev", "nebula", "nebula")
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = env::non_empty(env::DATA_DIR) {
        return PathBuf::from(dir);
    }
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| env::home_dir().unwrap_or_default().join(".nebula"))
}

pub fn db_path() -> PathBuf {
    data_dir().join("nebula.db")
}

/// User settings file (JSON). Lives beside the DB so `NEBULA_DATA_DIR`
/// isolates it for tests and parallel instances too.
pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn log_dir() -> PathBuf {
    // Tests and parallel instances override the data dir; keep their logs
    // beside their data instead of the real user's state dir.
    if env::non_empty(env::DATA_DIR).is_some() {
        return data_dir().join("state");
    }
    project_dirs()
        .map(|d| {
            d.state_dir()
                .map(|s| s.to_path_buf())
                .unwrap_or_else(|| d.data_dir().join("state"))
        })
        .unwrap_or_else(|| data_dir().join("state"))
}

pub fn daemon_log_path() -> PathBuf {
    log_dir().join("daemon.log")
}

pub fn tui_log_path() -> PathBuf {
    log_dir().join("tui.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Restores an env var to what it was when the guard was made, so a
    /// failed assertion can't leak an override into the next test.
    struct EnvRestore(&'static str, Option<std::ffi::OsString>);
    impl EnvRestore {
        fn new(var: &'static str) -> Self {
            Self(var, std::env::var_os(var))
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.1 {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }

    // One test owns both override vars: env is process-global, and splitting
    // this into several `#[test]`s would race them across threads.
    #[test]
    fn overrides_win_only_when_non_empty() {
        let _restore = (
            EnvRestore::new(crate::env::RUNTIME_DIR),
            EnvRestore::new(crate::env::DATA_DIR),
        );
        let runtime = std::env::temp_dir().join(format!("nebula-paths-rt-{}", std::process::id()));
        let data = std::env::temp_dir().join(format!("nebula-paths-data-{}", std::process::id()));

        std::env::set_var(crate::env::RUNTIME_DIR, &runtime);
        std::env::set_var(crate::env::DATA_DIR, &data);
        assert_eq!(runtime_dir(), runtime);
        assert_eq!(socket_path(), runtime.join("daemon.sock"));
        assert_eq!(pidfile_path(), runtime.join("daemon.pid"));
        assert_eq!(buildstamp_path(), runtime.join("daemon.build"));
        assert_eq!(data_dir(), data);
        assert_eq!(db_path(), data.join("nebula.db"));
        assert_eq!(config_path(), data.join("config.json"));
        // Logs follow the data override so isolated instances keep their
        // logs beside their state.
        assert_eq!(log_dir(), data.join("state"));
        assert_eq!(daemon_log_path(), data.join("state").join("daemon.log"));
        assert_eq!(tui_log_path(), data.join("state").join("tui.log"));

        std::env::set_var(crate::env::RUNTIME_DIR, "");
        std::env::set_var(crate::env::DATA_DIR, "");
        assert_ne!(runtime_dir(), runtime, "empty override must fall through");
        assert_ne!(data_dir(), data, "empty override must fall through");
        assert_ne!(log_dir(), data.join("state"));
        assert!(
            runtime_dir().is_absolute() && data_dir().is_absolute(),
            "defaults are absolute: {} / {}",
            runtime_dir().display(),
            data_dir().display()
        );
    }
}
