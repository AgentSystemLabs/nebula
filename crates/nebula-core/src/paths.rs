use std::path::PathBuf;

/// Runtime dir holding the socket + pidfile. Mode 0700 — this is the auth
/// boundary, same model as tmux. `NEBULA_RUNTIME_DIR` overrides (tests,
/// parallel instances).
pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEBULA_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("nebula");
        }
    }
    let uid = libc_geteuid();
    PathBuf::from(format!("/tmp/nebula-{uid}"))
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

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEBULA_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    directories::ProjectDirs::from("dev", "nebula", "nebula")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".nebula"))
}

pub fn db_path() -> PathBuf {
    data_dir().join("nebula.db")
}

pub fn log_dir() -> PathBuf {
    // Tests and parallel instances override the data dir; keep their logs
    // beside their data instead of the real user's state dir.
    if std::env::var("NEBULA_DATA_DIR").map(|d| !d.is_empty()).unwrap_or(false) {
        return data_dir().join("state");
    }
    directories::ProjectDirs::from("dev", "nebula", "nebula")
        .map(|d| d.state_dir().map(|s| s.to_path_buf()).unwrap_or_else(|| d.data_dir().join("state")))
        .unwrap_or_else(|| data_dir().join("state"))
}

pub fn daemon_log_path() -> PathBuf {
    log_dir().join("daemon.log")
}

pub fn tui_log_path() -> PathBuf {
    log_dir().join("tui.log")
}
