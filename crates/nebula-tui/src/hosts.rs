//! Recently-used `nebula ssh` destinations, backing the TUI's `Shift+H` picker.
//!
//! A plain JSON list in the data dir, most-recent first. `nebula ssh`
//! records every launch (typed invocations and picker handoffs both run
//! through it), the picker deletes entries. A missing or malformed file
//! reads as empty — the list is a convenience cache, never load-bearing.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Oldest entries fall off past this — the picker stays one screen tall.
pub(crate) const MAX_HOSTS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    /// ssh destination exactly as typed (e.g. `user@server`).
    pub host: String,
    /// Remote start directory, when one was given.
    #[serde(default)]
    pub path: Option<String>,
    /// Unix millis of the last connection, for the picker's "2h ago" label.
    #[serde(default)]
    pub last_used_ms: i64,
}

impl HostEntry {
    /// `host` or `host path` — the picker row and the reconnect identity.
    pub fn label(&self) -> String {
        match &self.path {
            Some(p) => format!("{} {}", self.host, p),
            None => self.host.clone(),
        }
    }
}

pub fn load() -> Vec<HostEntry> {
    load_from(&store_path())
}

/// Move `host` (+ start dir) to the front of the list, stamped now.
/// Best-effort: a failed write only costs the picker an entry.
pub fn record(host: &str, path: Option<&str>) {
    if let Err(err) = record_at(&store_path(), host, path, nebula_core::clock::now_ms()) {
        tracing::warn!(?err, "failed to record ssh host");
    }
}

/// Drop `entry` (matched by host + start dir) from the list.
pub fn remove(entry: &HostEntry) {
    if let Err(err) = remove_at(&store_path(), entry) {
        tracing::warn!(?err, "failed to remove ssh host");
    }
}

/// Parse a typed destination — `host` or `host dir` (the `nebula ssh`
/// argument shape; the dir may contain spaces). None when nothing was typed.
pub fn parse_destination(input: &str) -> Option<HostEntry> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let (host, rest) = match text.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (text, ""),
    };
    Some(HostEntry {
        host: host.to_string(),
        path: (!rest.is_empty()).then(|| rest.to_string()),
        last_used_ms: 0,
    })
}

/// "just now" / "5m ago" / "3h ago" / "12d ago"; empty when the entry
/// predates timestamps (or a clock went backwards).
pub fn ago_label(delta_ms: i64) -> String {
    if delta_ms < 0 {
        return String::new();
    }
    match delta_ms / 1000 {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

pub(crate) fn store_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = HOSTS_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    nebula_core::paths::data_dir().join(nebula_core::paths::SSH_HOSTS_FILE_NAME)
}

/// Entry by entry, so one unreadable entry costs only itself.
fn load_from(path: &Path) -> Vec<HostEntry> {
    nebula_core::settings::read_list(path).0
}

fn record_at(store: &Path, host: &str, path: Option<&str>, now_ms: i64) -> std::io::Result<()> {
    let mut hosts = load_from(store);
    hosts.retain(|e| !(e.host == host && e.path.as_deref() == path));
    hosts.insert(
        0,
        HostEntry {
            host: host.to_string(),
            path: path.map(str::to_string),
            last_used_ms: now_ms,
        },
    );
    hosts.truncate(MAX_HOSTS);
    save_to(store, &hosts)
}

fn remove_at(store: &Path, entry: &HostEntry) -> std::io::Result<()> {
    let mut hosts = load_from(store);
    hosts.retain(|e| !(e.host == entry.host && e.path == entry.path));
    save_to(store, &hosts)
}

fn save_to(store: &Path, hosts: &[HostEntry]) -> std::io::Result<()> {
    let value = serde_json::to_value(hosts)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    nebula_core::settings::write_json(store, &value)
}

#[cfg(test)]
thread_local! {
    static HOSTS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test hook (the `with_config_path` pattern): route this thread's host
/// store at `path` for the duration of `f`.
#[cfg(test)]
pub fn with_hosts_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    HOSTS_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssh_hosts.json");
        (dir, path)
    }

    #[test]
    fn missing_or_malformed_file_reads_empty() {
        let (_dir, path) = store();
        assert!(load_from(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn record_moves_to_front_and_restamps() {
        let (_dir, path) = store();
        record_at(&path, "a@one", None, 1).unwrap();
        record_at(&path, "b@two", None, 2).unwrap();
        record_at(&path, "a@one", None, 3).unwrap();
        let hosts = load_from(&path);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].host, "a@one");
        assert_eq!(hosts[0].last_used_ms, 3);
        assert_eq!(hosts[1].host, "b@two");
    }

    #[test]
    fn same_host_different_dir_is_a_separate_entry() {
        let (_dir, path) = store();
        record_at(&path, "a@one", None, 1).unwrap();
        record_at(&path, "a@one", Some("/srv/app"), 2).unwrap();
        let hosts = load_from(&path);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].path.as_deref(), Some("/srv/app"));
        assert_eq!(hosts[1].path, None);
    }

    #[test]
    fn list_caps_at_max_hosts() {
        let (_dir, path) = store();
        for i in 0..(MAX_HOSTS + 5) {
            record_at(&path, &format!("h{i}"), None, i as i64).unwrap();
        }
        let hosts = load_from(&path);
        assert_eq!(hosts.len(), MAX_HOSTS);
        assert_eq!(hosts[0].host, format!("h{}", MAX_HOSTS + 4), "newest kept");
    }

    #[test]
    fn remove_matches_host_and_dir() {
        let (_dir, path) = store();
        record_at(&path, "a@one", None, 1).unwrap();
        record_at(&path, "a@one", Some("/srv"), 2).unwrap();
        let hosts = load_from(&path);
        remove_at(&path, &hosts[0]).unwrap();
        let left = load_from(&path);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, None, "the dir-less twin survives");
    }

    #[test]
    fn an_unreadable_entry_costs_only_itself() {
        let (_dir, path) = store();
        std::fs::write(&path, r#"[{"host": 7}, {"host": "ok@box"}]"#).unwrap();
        let hosts = load_from(&path);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host, "ok@box");
    }

    #[test]
    fn entries_without_timestamp_deserialize() {
        let (_dir, path) = store();
        std::fs::write(&path, r#"[{"host": "old@box"}]"#).unwrap();
        let hosts = load_from(&path);
        assert_eq!(hosts[0].host, "old@box");
        assert_eq!(hosts[0].last_used_ms, 0);
        assert_eq!(hosts[0].path, None);
    }

    #[test]
    fn ago_labels() {
        assert_eq!(ago_label(-5), "");
        assert_eq!(ago_label(30_000), "just now");
        assert_eq!(ago_label(5 * 60_000), "5m ago");
        assert_eq!(ago_label(3 * 3_600_000), "3h ago");
        assert_eq!(ago_label(12 * 86_400_000), "12d ago");
    }

    #[test]
    fn parse_destination_splits_host_and_dir() {
        assert_eq!(parse_destination("  "), None);
        let plain = parse_destination("root@db").unwrap();
        assert_eq!(plain.host, "root@db");
        assert_eq!(plain.path, None);
        let with_dir = parse_destination(" root@db  /srv/my app ").unwrap();
        assert_eq!(with_dir.host, "root@db");
        assert_eq!(with_dir.path.as_deref(), Some("/srv/my app"));
    }

    #[test]
    fn label_includes_dir() {
        let entry = HostEntry {
            host: "a@one".into(),
            path: Some("/srv".into()),
            last_used_ms: 0,
        };
        assert_eq!(entry.label(), "a@one /srv");
    }
}
