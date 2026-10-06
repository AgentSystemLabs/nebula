use crate::env;
use std::path::{Path, PathBuf};

/// The settings files' names in the data dir. Their loaders, the SETTINGS
/// BUNDLE and `nebula config path` all spell them from here.
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const CONFIG_LOCAL_FILE_NAME: &str = "config.local.json";
pub const PRESETS_FILE_NAME: &str = "agent_presets.json";
pub const SSH_HOSTS_FILE_NAME: &str = "ssh_hosts.json";

/// Where the runtime dir lands when neither override nor `XDG_RUNTIME_DIR`
/// is set. World-writable, so the per-uid subdir is what carries mode 0700.
const FALLBACK_RUNTIME_ROOT: &str = "/tmp";
const APP_DIR_NAME: &str = "nebula-pilot";

/// Runtime dir holding the socket + pidfile. Mode 0700 — this is the auth
/// boundary, same model as tmux. `NEBULA_RUNTIME_DIR` overrides (tests,
/// parallel instances).
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::non_empty(env::RUNTIME_DIR) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env::non_empty("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join(APP_DIR_NAME);
    }
    let uid = libc_geteuid();
    Path::new(FALLBACK_RUNTIME_ROOT).join(format!("{APP_DIR_NAME}-{uid}"))
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

/// `<pid> <version>` of a daemon that can restart in place onto a new
/// binary, keeping its sessions. A daemon from before IN-PLACE RESTARTS
/// never writes it, which is how a client knows not to ask.
pub fn restart_capability_path() -> PathBuf {
    runtime_dir().join("daemon.restart")
}

/// A client's request for an IN-PLACE RESTART: which binary to restart
/// onto. The daemon reads and removes it when signalled.
pub fn restart_request_path() -> PathBuf {
    runtime_dir().join("restart.request")
}

/// How the last IN-PLACE RESTART went, written by whichever image of the
/// daemon finished it: the old one on a refusal, the new one on success.
pub fn restart_result_path() -> PathBuf {
    runtime_dir().join("restart.result")
}

/// The live sessions an IN-PLACE RESTART carries across the exec, read
/// and removed by the new image as it boots.
pub fn restart_state_path() -> PathBuf {
    runtime_dir().join("restart.state")
}

/// The platform's per-user dirs for this fork, separate from installed Nebula.
fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("dev", "nebula", APP_DIR_NAME)
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = env::non_empty(env::DATA_DIR) {
        return PathBuf::from(dir);
    }
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| env::home_dir().unwrap_or_default().join(".nebula-pilot"))
}

pub fn db_path() -> PathBuf {
    data_dir().join("nebula.db")
}

/// User settings file (JSON) — the portable layer, which backups and
/// `nebula ssh` carry. Lives beside the DB so `NEBULA_DATA_DIR` isolates it
/// for tests and parallel instances too; `NEBULA_CONFIG_FILE` moves this one
/// file alone (a leading `~/` is expanded, since a quoted value never is).
pub fn config_path() -> PathBuf {
    match env::non_empty(env::CONFIG_FILE) {
        Some(file) => match file.strip_prefix("~/") {
            Some(rest) => env::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(file),
        },
        None => data_dir().join(CONFIG_FILE_NAME),
    }
}

/// This machine's settings, layered over [`config_path`] key by key: never
/// exported, forwarded or overwritten by an import. Stays in the data dir
/// when `NEBULA_CONFIG_FILE` moves the portable file.
pub fn config_local_path() -> PathBuf {
    data_dir().join(CONFIG_LOCAL_FILE_NAME)
}

/// Claude Code's config dir: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_config_dir() -> Option<PathBuf> {
    env::non_empty(env::CLAUDE_CONFIG_DIR)
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(".claude")))
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let _lock = ENV_LOCK.lock().unwrap();
        let _restore = (
            EnvRestore::new(crate::env::RUNTIME_DIR),
            EnvRestore::new(crate::env::DATA_DIR),
            EnvRestore::new(crate::env::CONFIG_FILE),
        );
        std::env::remove_var(crate::env::CONFIG_FILE);
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
        assert_eq!(config_local_path(), data.join("config.local.json"));
        // Logs follow the data override so isolated instances keep their
        // logs beside their state.
        assert_eq!(log_dir(), data.join("state"));
        assert_eq!(daemon_log_path(), data.join("state").join("daemon.log"));
        assert_eq!(tui_log_path(), data.join("state").join("tui.log"));

        // The config file override moves that one file; the local layer and
        // everything else stay in the data dir.
        let dotfile = std::env::temp_dir().join("dotfiles").join("nebula.json");
        std::env::set_var(crate::env::CONFIG_FILE, &dotfile);
        assert_eq!(config_path(), dotfile);
        assert_eq!(config_local_path(), data.join("config.local.json"));
        assert_eq!(db_path(), data.join("nebula.db"));
        std::env::set_var(crate::env::CONFIG_FILE, "~/dotfiles/nebula.json");
        assert_eq!(
            config_path(),
            crate::env::home_dir()
                .unwrap()
                .join("dotfiles")
                .join("nebula.json")
        );
        std::env::set_var(crate::env::CONFIG_FILE, "");
        assert_eq!(config_path(), data.join("config.json"));

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

    #[test]
    fn defaults_are_isolated_from_upstream_nebula() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _restore = (
            EnvRestore::new(crate::env::RUNTIME_DIR),
            EnvRestore::new(crate::env::DATA_DIR),
            EnvRestore::new("XDG_RUNTIME_DIR"),
        );
        for var in [
            crate::env::RUNTIME_DIR,
            crate::env::DATA_DIR,
            "XDG_RUNTIME_DIR",
        ] {
            std::env::remove_var(var);
        }

        let runtime = runtime_dir();
        let data = data_dir();
        let logs = log_dir();

        assert!(
            runtime.to_string_lossy().contains(APP_DIR_NAME),
            "{runtime:?}"
        );
        assert!(data.to_string_lossy().contains(APP_DIR_NAME), "{data:?}");
        assert!(logs.to_string_lossy().contains(APP_DIR_NAME), "{logs:?}");
        assert_ne!(
            runtime,
            Path::new(FALLBACK_RUNTIME_ROOT).join(format!("nebula-{}", libc_geteuid()))
        );
        let upstream_data = directories::ProjectDirs::from("dev", "nebula", "nebula")
            .map(|d| d.data_dir().to_path_buf());
        assert_ne!(
            upstream_data,
            Some(data),
            "the fork must not reuse upstream Nebula's data directory"
        );
    }
}
